import CoreGraphics
import Foundation

/// One serial compositor and one pending *raw* packet, not an unbounded Task per
/// frame. Replacing a delta may break ancestry: the compositor then requests a
/// keyframe instead of ever drawing an incomplete desktop. A keyframe replaces
/// any pending work and establishes a new sequence baseline.
actor DesktopFrameWorker {
    nonisolated struct Job: Sendable {
        let data: Data
        let generation: Int
    }

    nonisolated struct Update: Sendable {
        let generation: Int
        let display: String
        let sequence: UInt64?
        let image: CGImage?
        let needsKeyframe: Bool
    }

    // Only admission metadata crosses executors. The actor is its sole writer;
    // synchronous raw enqueue checks it *before* an awaited meta can open the
    // fence. Otherwise a pre-meta packet could be buffered and decoded too late.
    private nonisolated final class Admission: @unchecked Sendable {
        private let lock = NSLock()
        private var generation = 0
        private var ready = false

        func set(generation: Int, ready: Bool) {
            lock.lock()
            defer { lock.unlock() }
            self.generation = generation
            self.ready = ready
        }

        func accepts(_ generation: Int) -> Bool {
            lock.lock()
            defer { lock.unlock() }
            return ready && self.generation == generation
        }
    }

    private nonisolated let admission = Admission()
    private let packets: AsyncStream<Job>
    private nonisolated let continuation: AsyncStream<Job>.Continuation
    private let compositor: DesktopFrameCompositor
    private var generation = 0
    private var selectedDisplay: String?
    private var displayFence = DesktopDisplayFence()
    private var running = false

    init(imageByteBudget: Int = 96 * 1024 * 1024) {
        (packets, continuation) = AsyncStream.makeStream(bufferingPolicy: .bufferingNewest(1))
        compositor = DesktopFrameCompositor(imageByteBudget: imageByteBudget)
    }

    /// Nonisolated, bounded enqueue only. Decoding, JPEG work and CG snapshots
    /// happen exclusively inside run(), on this actor's executor.
    @discardableResult
    nonisolated func submit(_ data: Data, generation: Int) -> Bool {
        guard admission.accepts(generation) else { return false }
        if case .dropped = continuation.yield(Job(data: data, generation: generation)) { return true }
        return false
    }

    nonisolated func finish() { continuation.finish() }

    /// Tokens also change on display selection and transport invalidation, so a
    /// queued old packet cannot seed a new stream even when sequence numbers reset.
    func configure(generation: Int, display: String?, newStream: Bool) -> CGImage? {
        guard generation >= self.generation else { return nil }
        self.generation = generation
        admission.set(generation: generation, ready: false)
        selectedDisplay = display
        if newStream || display == nil {
            displayFence.reset()
            compositor.beginStream()
        }
        if let display {
            compositor.selectDisplay(display)
            displayFence.request(display)
            return compositor.image(for: display)
        }
        return nil
    }

    func acknowledge(_ display: String, generation: Int) {
        guard generation == self.generation, displayFence.acknowledge(display) else { return }
        // Cached previews remain useful until this stream's first keyframe.
        // Reset sequence ancestry, but do not synchronously decode on MainActor.
        compositor.beginStream()
        admission.set(generation: generation, ready: true)
    }

    func run(publish: @escaping @MainActor @Sendable (Update) -> Void) async {
        guard !running else { return }
        running = true
        defer { running = false }
        for await job in packets {
            guard !Task.isCancelled else { return }
            guard job.generation == generation, let display = selectedDisplay else { continue }
            let update: Update
            do {
                let packet = try DesktopPacket.decode(job.data)
                guard packet.display == display, displayFence.accepts(packet.display) else { continue }
                guard try compositor.apply(packet) else { continue }
                update = Update(generation: generation, display: display, sequence: packet.seq,
                                image: compositor.image(for: display), needsKeyframe: false)
            } catch {
                update = Update(generation: generation, display: display, sequence: nil,
                                image: nil, needsKeyframe: true)
            }
            // This is the only executor hop for a frame. The receiver must also
            // compare the token: selection may change while this hop is queued.
            await publish(update)
        }
    }
}
