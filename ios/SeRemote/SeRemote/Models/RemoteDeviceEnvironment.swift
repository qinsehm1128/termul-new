import SwiftUI

private struct RemoteDeviceLinkKey: EnvironmentKey {
    static let defaultValue: RemoteLink? = nil
}

extension EnvironmentValues {
    var remoteDeviceLink: RemoteLink? {
        get { self[RemoteDeviceLinkKey.self] }
        set { self[RemoteDeviceLinkKey.self] = newValue }
    }
}
