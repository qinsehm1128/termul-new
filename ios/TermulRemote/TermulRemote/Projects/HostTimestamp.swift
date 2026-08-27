import Foundation

enum HostTimestamp {
    private static let fractional: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter
    }()

    private static let basic: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime]
        return formatter
    }()

    static func date(from value: String?) -> Date? {
        guard let value, !value.isEmpty else { return nil }
        return fractional.date(from: value) ?? basic.date(from: value)
    }

    static func relative(from value: String?) -> String? {
        guard let date = date(from: value) else { return nil }
        return date.formatted(.relative(presentation: .named, unitsStyle: .abbreviated))
    }

    static func folderName(from path: String?) -> String? {
        guard let path, !path.isEmpty else { return nil }
        let name = path.split(separator: "/").last.map(String.init)
        return name?.isEmpty == false ? name : path
    }
}
