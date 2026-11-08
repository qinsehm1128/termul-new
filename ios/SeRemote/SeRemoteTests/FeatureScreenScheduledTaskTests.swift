import Foundation
import Testing
@testable import SeRemote

@MainActor
struct FeatureScreenScheduledTaskTests {
    @Test func cronSummaryAlwaysIncludesExpressionAndOriginalTimezone() {
        let summary = ScheduledTaskScheduleFormatter.summary(.cron(
            expression: "0 9 * * 1-5", timezone: "America/New_York"))
        #expect(summary.contains("0 9 * * 1-5"))
        #expect(summary.contains("America/New_York"))
        #expect(summary.contains("Cron"))
    }

    @Test func cronUTCIsStillExplicit() {
        #expect(ScheduledTaskScheduleFormatter.summary(
            .cron(expression: "*/15 * * * *", timezone: "UTC")).contains("UTC"))
    }

    @Test func intervalIncludesDurationAnchorAndTimezone() {
        let anchor = "2026-11-08T01:00:00Z"
        let summary = ScheduledTaskScheduleFormatter.summary(.interval(everySeconds: 3_600, anchorAt: anchor))
        #expect(summary.contains(ScheduledTaskScheduleFormatter.duration(3_600)))
        #expect(summary.contains(ScheduledTaskScheduleFormatter.timestamp(anchor)))
        #expect(summary.contains("UTC"))
    }

    @Test func oneShotIncludesTimestampAndTimezone() {
        let time = "2026-11-09T10:30:00.123Z"
        let summary = ScheduledTaskScheduleFormatter.summary(.at(at: time))
        #expect(summary.contains(ScheduledTaskScheduleFormatter.timestamp(time)))
        #expect(summary.contains("UTC"))
        #expect(summary.contains("Once"))
    }

    @Test func timezoneRemainsExplicitWithoutAValidAnchor() {
        #expect(ScheduledTaskScheduleFormatter.summary(.interval(everySeconds: 60, anchorAt: "")).contains("UTC"))
        #expect(ScheduledTaskScheduleFormatter.summary(.at(at: "future-host-time")).contains("UTC"))
    }

    @Test func absoluteTimeNormalizesOffsetsRatherThanUsingPhoneTimezone() {
        #expect(ScheduledTaskScheduleFormatter.timestamp("2026-11-09T10:30:00+08:00") ==
            ScheduledTaskScheduleFormatter.timestamp("2026-11-09T02:30:00Z"))
    }

    @Test func unknownAndInvalidWireDataRemainVisible() {
        #expect(ScheduledTaskScheduleFormatter.summary(.unknown("solar")).contains("solar"))
        #expect(ScheduledTaskScheduleFormatter.timestamp("future-host-time") == "future-host-time")
        #expect(!ScheduledTaskScheduleFormatter.timestamp(nil).isEmpty)
    }

    @Test func durationUsesLargestExactUnitWithoutRounding() {
        #expect(ScheduledTaskScheduleFormatter.duration(86_400) == String(localized: "1 day"))
        #expect(ScheduledTaskScheduleFormatter.duration(7_200) == String(localized: "\(UInt64(2)) hours"))
        #expect(ScheduledTaskScheduleFormatter.duration(60) == String(localized: "1 minute"))
        #expect(ScheduledTaskScheduleFormatter.duration(90) == String(localized: "\(UInt64(90)) seconds"))
        #expect(ScheduledTaskScheduleFormatter.duration(0) == String(localized: "\(UInt64(0)) seconds"))
        #expect(!ScheduledTaskScheduleFormatter.duration(UInt64.max).isEmpty)
    }
}
