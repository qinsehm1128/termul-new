use std::str::FromStr;

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use cron::Schedule;

use super::models::{SchedulePreviewV1, ScheduleSpecV1};

const MAX_PREVIEW_OCCURRENCES: usize = 32;
const MIN_INTERVAL_SECONDS: u64 = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleError {
    InvalidCron(String),
    InvalidTimezone(String),
    InvalidTimestamp(String),
    InvalidInterval(String),
    NoFutureOccurrence,
}

impl std::fmt::Display for ScheduleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCron(detail) => write!(formatter, "invalid cron expression: {detail}"),
            Self::InvalidTimezone(detail) => write!(formatter, "invalid IANA timezone: {detail}"),
            Self::InvalidTimestamp(detail) => {
                write!(formatter, "invalid RFC3339 timestamp: {detail}")
            }
            Self::InvalidInterval(detail) => write!(formatter, "invalid interval: {detail}"),
            Self::NoFutureOccurrence => write!(formatter, "schedule has no future occurrence"),
        }
    }
}

impl std::error::Error for ScheduleError {}

pub fn preview_schedule(
    schedule: &ScheduleSpecV1,
    now: DateTime<Utc>,
    count: usize,
) -> Result<SchedulePreviewV1, ScheduleError> {
    let normalized = normalize_schedule(schedule)?;
    let next_run_times = occurrences_after(&normalized, now, count.min(MAX_PREVIEW_OCCURRENCES))?
        .into_iter()
        .map(|value| value.to_rfc3339())
        .collect();
    Ok(SchedulePreviewV1 {
        normalized,
        next_run_times,
    })
}

pub fn normalize_schedule(schedule: &ScheduleSpecV1) -> Result<ScheduleSpecV1, ScheduleError> {
    match schedule {
        ScheduleSpecV1::Cron {
            expression,
            timezone,
        } => {
            let expression = expression.split_whitespace().collect::<Vec<_>>().join(" ");
            let timezone = timezone.trim().to_string();
            parse_cron(&expression)?;
            timezone
                .parse::<Tz>()
                .map_err(|_| ScheduleError::InvalidTimezone(timezone.clone()))?;
            Ok(ScheduleSpecV1::Cron {
                expression,
                timezone,
            })
        }
        ScheduleSpecV1::Interval {
            every_seconds,
            anchor_at,
        } => {
            if *every_seconds < MIN_INTERVAL_SECONDS {
                return Err(ScheduleError::InvalidInterval(format!(
                    "everySeconds must be at least {MIN_INTERVAL_SECONDS}"
                )));
            }
            let anchor = parse_timestamp(anchor_at)?;
            Ok(ScheduleSpecV1::Interval {
                every_seconds: *every_seconds,
                anchor_at: anchor.to_rfc3339(),
            })
        }
        ScheduleSpecV1::At { at } => Ok(ScheduleSpecV1::At {
            at: parse_timestamp(at)?.to_rfc3339(),
        }),
    }
}

pub fn next_after(
    schedule: &ScheduleSpecV1,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, ScheduleError> {
    Ok(occurrences_after(schedule, after, 1)?.into_iter().next())
}

pub fn occurrences_after(
    schedule: &ScheduleSpecV1,
    after: DateTime<Utc>,
    count: usize,
) -> Result<Vec<DateTime<Utc>>, ScheduleError> {
    if count == 0 {
        return Ok(Vec::new());
    }
    match normalize_schedule(schedule)? {
        ScheduleSpecV1::Cron {
            expression,
            timezone,
        } => {
            let cron = parse_cron(&expression)?;
            let timezone = timezone
                .parse::<Tz>()
                .map_err(|_| ScheduleError::InvalidTimezone(timezone.clone()))?;
            let after_local = after.with_timezone(&timezone);
            Ok(cron
                .after(&after_local)
                .take(count)
                .map(|value| value.with_timezone(&Utc))
                .collect())
        }
        ScheduleSpecV1::Interval {
            every_seconds,
            anchor_at,
        } => {
            let anchor = parse_timestamp(&anchor_at)?;
            let interval = i64::try_from(every_seconds).map_err(|_| {
                ScheduleError::InvalidInterval("everySeconds exceeds i64".to_string())
            })?;
            let elapsed = after.signed_duration_since(anchor).num_seconds();
            let periods = if elapsed < 0 {
                0
            } else {
                elapsed.div_euclid(interval) + 1
            };
            let first = anchor
                .checked_add_signed(Duration::seconds(periods.saturating_mul(interval)))
                .ok_or_else(|| {
                    ScheduleError::InvalidInterval(
                        "next occurrence overflows timestamp".to_string(),
                    )
                })?;
            let mut values = Vec::with_capacity(count);
            for offset in 0..count {
                let multiplier = i64::try_from(offset)
                    .ok()
                    .and_then(|offset| offset.checked_mul(interval))
                    .ok_or_else(|| {
                        ScheduleError::InvalidInterval(
                            "preview occurrence overflows interval".to_string(),
                        )
                    })?;
                values.push(
                    first
                        .checked_add_signed(Duration::seconds(multiplier))
                        .ok_or_else(|| {
                            ScheduleError::InvalidInterval(
                                "preview occurrence overflows timestamp".to_string(),
                            )
                        })?,
                );
            }
            Ok(values)
        }
        ScheduleSpecV1::At { at } => {
            let at = parse_timestamp(&at)?;
            Ok(if at > after { vec![at] } else { Vec::new() })
        }
    }
}

fn parse_timestamp(value: &str) -> Result<DateTime<Utc>, ScheduleError> {
    DateTime::parse_from_rfc3339(value.trim())
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| ScheduleError::InvalidTimestamp(error.to_string()))
}

fn parse_cron(expression: &str) -> Result<Schedule, ScheduleError> {
    let fields = expression.split_whitespace().collect::<Vec<_>>();
    let expanded = match fields.len() {
        5 => format!("0 {expression} *"),
        6 => format!("{expression} *"),
        7 => expression.to_string(),
        count => {
            return Err(ScheduleError::InvalidCron(format!(
                "expected 5, 6, or 7 fields; got {count}"
            )))
        }
    };
    Schedule::from_str(&expanded).map_err(|error| ScheduleError::InvalidCron(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-20T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn five_field_cron_uses_explicit_timezone() {
        let preview = preview_schedule(
            &ScheduleSpecV1::Cron {
                expression: "0 9 * * *".to_string(),
                timezone: "Asia/Shanghai".to_string(),
            },
            now(),
            2,
        )
        .unwrap();
        assert_eq!(
            preview.next_run_times,
            vec!["2026-08-20T01:00:00+00:00", "2026-08-21T01:00:00+00:00"]
        );
    }

    #[test]
    fn interval_is_anchored_and_strictly_after_reference() {
        let values = occurrences_after(
            &ScheduleSpecV1::Interval {
                every_seconds: 3600,
                anchor_at: "2026-08-19T23:30:00Z".to_string(),
            },
            now(),
            2,
        )
        .unwrap();
        assert_eq!(values[0].to_rfc3339(), "2026-08-20T00:30:00+00:00");
        assert_eq!(values[1].to_rfc3339(), "2026-08-20T01:30:00+00:00");
    }

    #[test]
    fn one_shot_has_no_occurrence_after_it_passes() {
        assert!(occurrences_after(
            &ScheduleSpecV1::At {
                at: "2026-08-19T23:00:00Z".to_string(),
            },
            now(),
            1,
        )
        .unwrap()
        .is_empty());
    }
}
