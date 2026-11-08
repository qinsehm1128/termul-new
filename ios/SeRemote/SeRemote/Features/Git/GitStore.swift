import Foundation
import Observation

enum GitLoadState: Equatable, Sendable {
    case idle
    case loading
    case loaded
    case empty
    case failed(String)
}

@Observable
@MainActor
final class GitStore {
    private(set) var state: GitLoadState = .idle
    private(set) var diffState: GitLoadState = .idle
    private(set) var cwd: String?
    private(set) var status: [GitStatusDetailWire] = []
    private(set) var commits: [GitCommitWire] = []
    private(set) var context: GitCommitContextWire?
    private(set) var branches: [GitBranch] = []
    private(set) var stashes: [GitStashInfoWire] = []
    private(set) var rawDiff = ""
    private(set) var diff = UnifiedDiff(files: [])
    private(set) var diffPath: String?
    private(set) var diffStaged = false

    @ObservationIgnored private var client: (any GitReading)?
    @ObservationIgnored private var refreshRevision = 0
    @ObservationIgnored private var diffRevision = 0

    func attach(http: HostHTTP) {
        attach(client: GitClient(http: http))
    }

    /// Also allows deterministic, transport-free store tests.
    func attach(client: any GitReading) {
        self.client = client
        refreshRevision += 1
        clearSnapshot()
        cwd = nil
        state = .idle
        clearDiff()
    }

    func refresh(cwd: String) async {
        guard let client else { return }
        refreshRevision += 1
        let revision = refreshRevision
        if self.cwd != cwd { clearSnapshot() }
        self.cwd = cwd
        // A refreshed index may invalidate any previously selected diff.
        clearDiff()
        state = .loading
        HostLog.session.info("Git read refresh started")
        do {
            let status = try await client.status(cwd: cwd)
            let commits = try await client.log(cwd: cwd, limit: nil)
            let context = try await client.commitContext(cwd: cwd)
            let branches = try await client.branches(cwd: cwd)
            let stashes = try await client.stashes(cwd: cwd)
            try Task.checkCancellation()
            guard revision == refreshRevision else { return }
            self.status = status
            self.commits = commits
            self.context = context
            self.branches = branches
            self.stashes = stashes
            // Empty describes the worktree; history/branches still remain usable.
            state = status.isEmpty ? .empty : .loaded
            HostLog.session.info("Git read refresh completed")
        } catch {
            guard revision == refreshRevision else { return }
            if error is CancellationError {
                state = .idle
            } else {
                state = .failed(error.localizedDescription)
                HostLog.session.error("Git read refresh failed")
            }
        }
    }

    func refresh() async {
        guard let cwd else { return }
        await refresh(cwd: cwd)
    }

    func loadDiff(path: String, staged: Bool = false) async {
        guard let client, let cwd else { return }
        clearDiff()
        let revision = diffRevision
        diffPath = path
        diffStaged = staged
        diffState = .loading
        HostLog.session.info("Git diff read started")
        do {
            let text = try await client.diff(cwd: cwd, path: path, staged: staged)
            try Task.checkCancellation()
            guard revision == diffRevision else { return }
            rawDiff = text
            diff = UnifiedDiff.parse(text)
            diffState = text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? .empty : .loaded
            HostLog.session.info("Git diff read completed")
        } catch {
            guard revision == diffRevision else { return }
            if error is CancellationError {
                diffState = .idle
            } else {
                diffState = .failed(error.localizedDescription)
                HostLog.session.error("Git diff read failed")
            }
        }
    }

    private func clearSnapshot() {
        status = []
        commits = []
        context = nil
        branches = []
        stashes = []
    }

    private func clearDiff() {
        diffRevision += 1
        rawDiff = ""
        diff = UnifiedDiff(files: [])
        diffPath = nil
        diffStaged = false
        diffState = .idle
    }
}
