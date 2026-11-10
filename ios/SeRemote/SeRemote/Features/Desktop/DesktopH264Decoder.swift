import CoreGraphics
import CoreMedia
import CoreVideo
import Foundation
import VideoToolbox

/// Consecutive decode failures since the last good IDR. Most deltas of an
/// incompatible stream (a 1080p SPS on a 720p device) still decode, so a delta
/// success must not hide the incompatibility: only a successful keyframe
/// resets the streak, and three failures since the last IDR force JPEG.
nonisolated struct H264FailureTracker {
    static let maxConsecutiveFailures = 3
    private(set) var consecutiveFailures = 0

    mutating func reset() { consecutiveFailures = 0 }

    mutating func recordSuccess(isKey: Bool) {
        if isKey { consecutiveFailures = 0 }
    }

    /// Returns true when this failure completes the 3-strike fallback.
    mutating func recordFailure() -> Bool {
        consecutiveFailures += 1
        return consecutiveFailures >= Self.maxConsecutiveFailures
    }
}

/// Decodes one frozen P1 H.264 stream. Owned exclusively by DesktopFrameWorker,
/// so no locking is needed around its session; only the VideoToolbox output box
/// crosses into the decoder's callback queue.
nonisolated final class DesktopH264Decoder {
    nonisolated struct Outcome {
        let image: CGImage?
        let requestKeyframe: Bool
        let fallbackToJPEG: Bool
    }

    private enum DecodeError: Error {
        case unsupported
        case formatDescription
        case session
        case sampleBuffer
        case decode
        case conversion
    }

    private final class OutputBox: @unchecked Sendable {
        private let lock = NSLock()
        private var status: OSStatus = -1
        private var buffer: CVImageBuffer?

        func store(status: OSStatus, buffer: CVImageBuffer?) {
            lock.lock()
            defer { lock.unlock() }
            self.status = status
            self.buffer = buffer
        }

        func take() -> (OSStatus, CVImageBuffer?) {
            lock.lock()
            defer { lock.unlock() }
            return (status, buffer)
        }
    }

    static var isHardwareDecodeSupported: Bool {
        VTIsHardwareDecodeSupported(kCMVideoCodecType_H264)
    }

    private var session: VTDecompressionSession?
    private var formatDescription: CMVideoFormatDescription?
    private var parameterSets: [Data] = []
    private var lastSequence: UInt64?
    private var waitingForKeyframe = true
    private var keyframeRequested = false
    private var failures = H264FailureTracker()
    private var forceFallback = false

    /// New connection, display switch or explicit restart: try H.264 again.
    func reset() {
        invalidateSession()
        parameterSets = []
        lastSequence = nil
        waitingForKeyframe = true
        keyframeRequested = false
        failures.reset()
        forceFallback = false
    }

    /// Host or this client gave up on H.264: free the session and ignore further
    /// H.264 packets until the next reset.
    func disable() {
        invalidateSession()
        parameterSets = []
        lastSequence = nil
        waitingForKeyframe = true
        keyframeRequested = false
        failures.reset()
        forceFallback = true
    }

    func decode(_ packet: DesktopH264Packet) -> Outcome {
        if forceFallback {
            return Outcome(image: nil, requestKeyframe: false, fallbackToJPEG: true)
        }

        if packet.isKey {
            guard rebuildIfNeeded(packet.parameterSets) else { return failure() }
            waitingForKeyframe = false
            keyframeRequested = false
        } else {
            // First frame must be a key. Gaps (including a coalesced backlog)
            // invalidate ancestry: drop deltas until the next key.
            guard !waitingForKeyframe, session != nil, let previous = lastSequence,
                  previous != UInt64.max, packet.seq == previous + 1 else {
                return keyframeRequired()
            }
        }

        guard let session, let formatDescription else { return failure() }
        do {
            let image = try decodeSample(
                packet.sample, session: session, formatDescription: formatDescription,
                presentationTime: CMTime(value: Int64(bitPattern: packet.seq), timescale: 30))
            lastSequence = packet.seq
            // Only a decodable IDR proves the stream matches this decoder;
            // a delta that happens to decode must not clear the streak.
            failures.recordSuccess(isKey: packet.isKey)
            waitingForKeyframe = false
            return Outcome(image: image, requestKeyframe: false, fallbackToJPEG: false)
        } catch {
            return failure()
        }
    }

    private func keyframeRequired() -> Outcome {
        waitingForKeyframe = true
        let request = !keyframeRequested
        keyframeRequested = true
        return Outcome(image: nil, requestKeyframe: request, fallbackToJPEG: false)
    }

    private func failure() -> Outcome {
        invalidateSession()
        waitingForKeyframe = true
        if failures.recordFailure() {
            forceFallback = true
            return Outcome(image: nil, requestKeyframe: false, fallbackToJPEG: true)
        }
        let request = !keyframeRequested
        keyframeRequested = true
        return Outcome(image: nil, requestKeyframe: request, fallbackToJPEG: false)
    }

    private func rebuildIfNeeded(_ sets: [Data]) -> Bool {
        guard sets.count == 2 else { return false }
        if session != nil, sets == parameterSets { return true }
        invalidateSession()
        do {
            try makeSession(sets)
            parameterSets = sets
            return true
        } catch {
            parameterSets = []
            return false
        }
    }

    private func makeSession(_ sets: [Data]) throws {
        var format: CMVideoFormatDescription?
        let formatStatus: OSStatus = sets[0].withUnsafeBytes { spsRaw in
            sets[1].withUnsafeBytes { ppsRaw in
                guard let sps = spsRaw.bindMemory(to: UInt8.self).baseAddress,
                      let pps = ppsRaw.bindMemory(to: UInt8.self).baseAddress else {
                    return OSStatus(-1)
                }
                var pointers: [UnsafePointer<UInt8>] = [sps, pps]
                var sizes: [Int] = [sets[0].count, sets[1].count]
                return CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    allocator: kCFAllocatorDefault, parameterSetCount: 2,
                    parameterSetPointers: &pointers, parameterSetSizes: &sizes,
                    nalUnitHeaderLength: 4, formatDescriptionOut: &format)
            }
        }
        guard formatStatus == noErr, let format else { throw DecodeError.formatDescription }

        let attributes: [CFString: Any] = [
            kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_32BGRA,
            kCVPixelBufferIOSurfacePropertiesKey: [:] as CFDictionary,
        ]
        var created: VTDecompressionSession?
        let sessionStatus = VTDecompressionSessionCreate(
            allocator: kCFAllocatorDefault, formatDescription: format,
            decoderSpecification: nil,
            imageBufferAttributes: attributes as CFDictionary,
            outputCallback: nil, decompressionSessionOut: &created)
        guard sessionStatus == noErr, let created else { throw DecodeError.session }
        session = created
        formatDescription = format
    }

    private func decodeSample(_ sample: Data, session: VTDecompressionSession,
                              formatDescription: CMVideoFormatDescription,
                              presentationTime: CMTime) throws -> CGImage {
        var blockBuffer: CMBlockBuffer?
        let blockStatus = sample.withUnsafeBytes { raw -> OSStatus in
            var block: CMBlockBuffer?
            let createStatus = CMBlockBufferCreateWithMemoryBlock(
                allocator: kCFAllocatorDefault, memoryBlock: nil,
                blockLength: sample.count, blockAllocator: kCFAllocatorDefault,
                customBlockSource: nil, offsetToData: 0, dataLength: sample.count,
                flags: 0, blockBufferOut: &block)
            guard createStatus == noErr, let block, let base = raw.baseAddress else {
                return createStatus == noErr ? OSStatus(-1) : createStatus
            }
            let copyStatus = CMBlockBufferReplaceDataBytes(
                with: base, blockBuffer: block,
                offsetIntoDestination: 0, dataLength: sample.count)
            blockBuffer = block
            return copyStatus
        }
        guard blockStatus == noErr, let blockBuffer else { throw DecodeError.sampleBuffer }

        var sampleBuffer: CMSampleBuffer?
        var timing = CMSampleTimingInfo(duration: .invalid, presentationTimeStamp: presentationTime,
                                        decodeTimeStamp: .invalid)
        var sampleSize = sample.count
        let sampleStatus = CMSampleBufferCreateReady(
            allocator: kCFAllocatorDefault, dataBuffer: blockBuffer,
            formatDescription: formatDescription, sampleCount: 1,
            sampleTimingEntryCount: 1, sampleTimingArray: &timing,
            sampleSizeEntryCount: 1, sampleSizeArray: &sampleSize,
            sampleBufferOut: &sampleBuffer)
        guard sampleStatus == noErr, let sampleBuffer else { throw DecodeError.sampleBuffer }

        let box = OutputBox()
        var infoFlags = VTDecodeInfoFlags()
        let decodeStatus = VTDecompressionSessionDecodeFrame(
            session, sampleBuffer: sampleBuffer, flags: [], infoFlagsOut: &infoFlags
        ) { status, _, imageBuffer, _, _, _ in
            box.store(status: status, buffer: imageBuffer)
        }
        guard decodeStatus == noErr else { throw DecodeError.decode }
        let (status, buffer) = box.take()
        guard status == noErr, let buffer else { throw DecodeError.decode }
        return try Self.makeImage(from: buffer)
    }

    private static func makeImage(from pixelBuffer: CVPixelBuffer) throws -> CGImage {
        guard CVPixelBufferGetPixelFormatType(pixelBuffer) == kCVPixelFormatType_32BGRA else {
            throw DecodeError.conversion
        }
        CVPixelBufferLockBaseAddress(pixelBuffer, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(pixelBuffer, .readOnly) }
        guard let base = CVPixelBufferGetBaseAddress(pixelBuffer) else { throw DecodeError.conversion }

        let width = CVPixelBufferGetWidth(pixelBuffer)
        let height = CVPixelBufferGetHeight(pixelBuffer)
        let sourceStride = CVPixelBufferGetBytesPerRow(pixelBuffer)
        let destinationStride = width * 4
        guard width > 0, height > 0, sourceStride >= destinationStride,
              let context = CGContext(
                data: nil, width: width, height: height,
                bitsPerComponent: 8, bytesPerRow: destinationStride,
                space: CGColorSpaceCreateDeviceRGB(),
                bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue
                    | CGBitmapInfo.byteOrder32Little.rawValue),
              let destination = context.data else {
            throw DecodeError.conversion
        }
        if sourceStride == destinationStride {
            memcpy(destination, base, destinationStride * height)
        } else {
            for row in 0..<height {
                memcpy(destination.advanced(by: row * destinationStride),
                       base.advanced(by: row * sourceStride), destinationStride)
            }
        }
        guard let image = context.makeImage() else { throw DecodeError.conversion }
        return image
    }

    private func invalidateSession() {
        if let session { VTDecompressionSessionInvalidate(session) }
        session = nil
        formatDescription = nil
    }
}
