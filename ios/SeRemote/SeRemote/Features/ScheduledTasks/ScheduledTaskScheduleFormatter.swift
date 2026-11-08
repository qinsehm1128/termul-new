import Foundation

/// Presentation only: never interprets cron in the phone's local timezone.
/// Absolute timestamps are normalized to UTC; cron keeps the host's IANA zone.
nonisolated enum ScheduledTaskScheduleFormatter {
    static func summary(_ schedule: ScheduleSpecWire) -> String {
        switch schedule {
        case .cron(let expression, let timezone):
            String(localized: "Cron \(expression) · \(timezone)")
        case .interval(let seconds, let anchor):
            String(localized: "Every \(duration(seconds)) · UTC · anchored \(timestamp(anchor))")
        case .at(let at):
            String(localized: "Once · UTC · \(timestamp(at))")
        case .unknown(let kind):
            String(localized: "Unknown schedule: \(kind) · timezone unavailable")
        }
    }

    static func duration(_ seconds: UInt64) -> String {
        if seconds == 86_400 { return String(localized: "1 day") }
        if seconds == 3_600 { return String(localized: "1 hour") }
        if seconds == 60 { return String(localized: "1 minute") }
        if seconds == 1 { return String(localized: "1 second") }
        if seconds > 0, seconds.isMultiple(of: 86_400) {
            return String(localized: "\(seconds / 86_400) days")
        }
        if seconds > 0, seconds.isMultiple(of: 3_600) {
            return String(localized: "\(seconds / 3_600) hours")
        }
        if seconds > 0, seconds.isMultiple(of: 60) {
            return String(localized: "\(seconds / 60) minutes")
        }
        return String(localized: "\(seconds) seconds")
    }

    static func timestamp(_ value: String?) -> String {
        guard let value, !value.isEmpty else { return String(localized: "Not scheduled") }
        let parser = ISO8601DateFormatter()
        parser.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        var date = parser.date(from: value)
        if date == nil {
            parser.formatOptions = [.withInternetDateTime]
            date = parser.date(from: value)
        }
        guard let date else { return value } // Preserve future host formats, never invent a time.
        let formatter = DateFormatter()
        formatter.dateStyle = .medium
        formatter.timeStyle = .medium
        formatter.timeZone = TimeZone(secondsFromGMT: 0)
        return formatter.string(from: date) + " UTC"
    }
}
