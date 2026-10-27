// SPDX-License-Identifier: CC0-1.0

//! A Kinesis data stream's RETENTION PERIOD, end to end over the wire: `CreateStream` ->
//! `IncreaseStreamRetentionPeriod` / `DecreaseStreamRetentionPeriod` ->
//! `DescribeStreamSummary` / `DescribeStream`.
//!
//! RULE: a retention change stores the `RetentionPeriodHours` its request carries on the
//! stream it addresses, so that is the window every later read of that stream reports. The
//! window is bounded at both ends — 24 hours at the low end and 8760 hours (365 days) at the
//! high end — and each operation only moves it STRICTLY in its own direction, so a request
//! outside the bounds, and one that does not move the window the operation's own way, are
//! refused with `InvalidArgumentException` and leave the stored window exactly as it was. The call is
//! served only while the stream is `ACTIVE` (`ResourceInUseException` otherwise) and only
//! for a stream this account and region hold (`ResourceNotFoundException` otherwise).
//!
//! The recorded flow this was derived from asks for one hour count past the ceiling on one
//! stream, which is one row of the tables below. The rows differ along what the rule
//! generalises over: the direction of the change, the hour count (both published bounds, one
//! past each of them, the default and values between), how many changes one stream takes,
//! the identifier the request resolves the stream by, and the status the stream is in when
//! the call arrives.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    retention_period_not_longer_message, retention_period_not_shorter_message,
    retention_period_too_long_message, retention_period_too_short_message,
    stream_not_found_message, DEFAULT_RETENTION_PERIOD_HOURS, MAX_RETENTION_PERIOD_HOURS,
    MIN_RETENTION_PERIOD_HOURS, STREAM_IDENTIFIER_REQUIRED_MESSAGE,
};

use crate::stream_tags::{error_code, error_message, past_activation, stream_arn, Emulator, REGION};

/// The operation a row calls: the two directions of the one rule.
#[derive(Debug, Clone, Copy)]
enum Direction {
    /// `IncreaseStreamRetentionPeriod`, which only lengthens the window.
    Increase,
    /// `DecreaseStreamRetentionPeriod`, which only shortens it.
    Decrease,
}

impl Direction {
    /// The operation name this direction is served by.
    fn operation(self) -> &'static str {
        match self {
            Self::Increase => "IncreaseStreamRetentionPeriod",
            Self::Decrease => "DecreaseStreamRetentionPeriod",
        }
    }
}

/// The `RetentionPeriodHours` the stream reports, read back through
/// `DescribeStreamSummary`.
async fn reported_retention(emulator: &Emulator, case: &str, name: &str) -> i64 {
    let described = emulator
        .ok(case, "DescribeStreamSummary", &json!({ "StreamName": name }))
        .await;
    described["StreamDescriptionSummary"]
        .get("RetentionPeriodHours")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| {
            panic!("case {case}: the summary must carry RetentionPeriodHours, got {described}")
        })
}

/// An accepted change is what every later read of the stream reports.
#[tokio::test]
async fn a_retention_change_is_what_a_read_reports() {
    // Each row is a sequence of changes applied to one stream, and the window the stream
    // must report after each of them. A freshly created stream reports the documented
    // default, so the first change of each row starts from there.
    let cases: Vec<(&str, Vec<(Direction, i32)>)> = vec![
        ("one-step-up", vec![(Direction::Increase, 48)]),
        (
            "to-the-ceiling",
            vec![(Direction::Increase, MAX_RETENTION_PERIOD_HOURS)],
        ),
        (
            "just-under-the-ceiling",
            vec![(Direction::Increase, MAX_RETENTION_PERIOD_HOURS - 1)],
        ),
        (
            "just-over-the-floor",
            vec![(Direction::Increase, MIN_RETENTION_PERIOD_HOURS + 1)],
        ),
        (
            "up-then-back-to-the-floor",
            vec![
                (Direction::Increase, 168),
                (Direction::Decrease, MIN_RETENTION_PERIOD_HOURS),
            ],
        ),
        (
            "up-in-steps-then-down-in-steps",
            vec![
                (Direction::Increase, 36),
                (Direction::Increase, 720),
                (Direction::Increase, MAX_RETENTION_PERIOD_HOURS),
                (Direction::Decrease, 8_000),
                (Direction::Decrease, 25),
            ],
        ),
    ];

    let emulator = Emulator::new();
    for (name, _) in &cases {
        emulator
            .ok(
                name,
                "CreateStream",
                &json!({ "StreamName": name, "ShardCount": 1 }),
            )
            .await;
    }
    tokio::time::sleep(past_activation()).await;

    for (name, changes) in &cases {
        assert_eq!(
            reported_retention(&emulator, name, name).await,
            i64::from(DEFAULT_RETENTION_PERIOD_HOURS),
            "case {name}: expected a new stream to report the default window"
        );
        for (direction, hours) in changes {
            let case = format!("{name}-{}-{hours}", direction.operation());
            // Either identifier resolves the stream, so the rows alternate between them.
            let body = if hours % 2 == 0 {
                json!({ "StreamName": name, "RetentionPeriodHours": hours })
            } else {
                json!({
                    "StreamARN": stream_arn(REGION, name),
                    "RetentionPeriodHours": hours,
                })
            };
            emulator.ok(&case, direction.operation(), &body).await;
            let reported = reported_retention(&emulator, &case, name).await;
            assert_eq!(
                reported,
                i64::from(*hours),
                "case {case}: expected the stream to report the window the change stored, \
                 got {reported}"
            );
        }
    }

    // The window a read reports is also the window the full description carries.
    for (name, changes) in &cases {
        let expected = changes
            .last()
            .map(|(_, hours)| i64::from(*hours))
            .unwrap_or_else(|| i64::from(DEFAULT_RETENTION_PERIOD_HOURS));
        let described = emulator
            .ok(name, "DescribeStream", &json!({ "StreamName": name }))
            .await;
        let reported = described["StreamDescription"]
            .get("RetentionPeriodHours")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| {
                panic!("case {name}: the description must carry RetentionPeriodHours, got {described}")
            });
        assert_eq!(
            reported, expected,
            "case {name}: expected the description to carry the stored window, got {reported}"
        );
    }
}

/// What the stream a refusal row addresses looks like when the call arrives.
#[derive(Debug, Clone, Copy)]
enum Subject {
    /// An `ACTIVE` stream whose window is the default the create gave it.
    Default,
    /// An `ACTIVE` stream whose window has already been lengthened to this many hours.
    Lengthened(i32),
    /// A stream whose create has not fallen due yet.
    Creating,
    /// No stream at all.
    Absent,
}

/// One refused change: the stream it addresses, the direction and hour count it asks for,
/// and the modelled refusal it must answer.
struct Refusal {
    case: &'static str,
    subject: Subject,
    direction: Direction,
    hours: i32,
    code: &'static str,
    /// The message the refusal must carry, built from the row's own stream and hour count.
    says: fn(&str, i32) -> String,
}

/// A refused change answers the modelled client error and leaves the stored window as it
/// was.
#[tokio::test]
async fn a_refused_retention_change_leaves_the_window_as_it_was() {
    let cases: Vec<Refusal> = vec![
        Refusal {
            case: "past-the-ceiling",
            subject: Subject::Default,
            direction: Direction::Increase,
            hours: MAX_RETENTION_PERIOD_HOURS + 1,
            code: "InvalidArgumentException",
            says: |_, hours| retention_period_too_long_message(hours),
        },
        Refusal {
            case: "far-past-the-ceiling",
            subject: Subject::Default,
            direction: Direction::Increase,
            hours: i32::MAX,
            code: "InvalidArgumentException",
            says: |_, hours| retention_period_too_long_message(hours),
        },
        Refusal {
            case: "decrease-past-the-ceiling",
            subject: Subject::Default,
            direction: Direction::Decrease,
            hours: MAX_RETENTION_PERIOD_HOURS + 1,
            code: "InvalidArgumentException",
            says: |_, hours| retention_period_too_long_message(hours),
        },
        Refusal {
            case: "below-the-floor",
            subject: Subject::Lengthened(48),
            direction: Direction::Decrease,
            hours: MIN_RETENTION_PERIOD_HOURS - 1,
            code: "InvalidArgumentException",
            says: |_, hours| retention_period_too_short_message(hours),
        },
        Refusal {
            case: "no-retention-at-all",
            subject: Subject::Lengthened(48),
            direction: Direction::Decrease,
            hours: 0,
            code: "InvalidArgumentException",
            says: |_, hours| retention_period_too_short_message(hours),
        },
        Refusal {
            case: "increase-below-the-floor",
            subject: Subject::Default,
            direction: Direction::Increase,
            hours: 1,
            code: "InvalidArgumentException",
            says: |_, hours| retention_period_too_short_message(hours),
        },
        Refusal {
            case: "increase-that-shortens",
            subject: Subject::Lengthened(72),
            direction: Direction::Increase,
            hours: 48,
            code: "InvalidArgumentException",
            says: |name, hours| retention_period_not_longer_message(name, hours, 72),
        },
        Refusal {
            case: "decrease-that-lengthens",
            subject: Subject::Default,
            direction: Direction::Decrease,
            hours: 48,
            code: "InvalidArgumentException",
            says: |name, hours| {
                retention_period_not_shorter_message(name, hours, DEFAULT_RETENTION_PERIOD_HOURS)
            },
        },
        Refusal {
            // "Must be more than the current retention period"
            // (`IncreaseStreamRetentionPeriodInput$RetentionPeriodHours`), so the window the
            // stream already carries moves it nowhere and is refused too.
            case: "increase-to-the-window-it-has",
            subject: Subject::Default,
            direction: Direction::Increase,
            hours: DEFAULT_RETENTION_PERIOD_HOURS,
            code: "InvalidArgumentException",
            says: |name, hours| {
                retention_period_not_longer_message(name, hours, DEFAULT_RETENTION_PERIOD_HOURS)
            },
        },
        Refusal {
            // The same for the other direction: "must be less than the current retention
            // period" (`DecreaseStreamRetentionPeriodInput$RetentionPeriodHours`).
            case: "decrease-to-the-window-it-has",
            subject: Subject::Lengthened(96),
            direction: Direction::Decrease,
            hours: 96,
            code: "InvalidArgumentException",
            says: |name, hours| retention_period_not_shorter_message(name, hours, 96),
        },
        Refusal {
            case: "still-creating",
            subject: Subject::Creating,
            direction: Direction::Increase,
            hours: 48,
            code: "ResourceInUseException",
            says: |_, _| "CREATING".to_string(),
        },
        Refusal {
            case: "no-such-stream",
            subject: Subject::Absent,
            direction: Direction::Increase,
            hours: 48,
            code: "ResourceNotFoundException",
            says: |name, _| stream_not_found_message(name),
        },
    ];

    let emulator = Emulator::new();
    for refusal in &cases {
        match refusal.subject {
            Subject::Default | Subject::Lengthened(_) => {
                emulator
                    .ok(
                        refusal.case,
                        "CreateStream",
                        &json!({ "StreamName": refusal.case, "ShardCount": 1 }),
                    )
                    .await;
            }
            Subject::Creating | Subject::Absent => {}
        }
    }
    tokio::time::sleep(past_activation()).await;

    for refusal in &cases {
        let case = refusal.case;
        if let Subject::Lengthened(hours) = refusal.subject {
            emulator
                .ok(
                    case,
                    "IncreaseStreamRetentionPeriod",
                    &json!({ "StreamName": case, "RetentionPeriodHours": hours }),
                )
                .await;
        }
        if let Subject::Creating = refusal.subject {
            emulator
                .ok(
                    case,
                    "CreateStream",
                    &json!({ "StreamName": case, "ShardCount": 1 }),
                )
                .await;
        }

        // A stream nothing holds, and one whose create is still due, report no window to
        // compare against.
        let before = match refusal.subject {
            Subject::Creating | Subject::Absent => None,
            _ => Some(reported_retention(&emulator, case, case).await),
        };

        let (status, response) = emulator
            .call(
                refusal.direction.operation(),
                &json!({ "StreamName": case, "RetentionPeriodHours": refusal.hours }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a client error, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            refusal.code,
            "case {case}: expected {}, got {response}",
            refusal.code
        );
        let says = (refusal.says)(case, refusal.hours);
        let message = error_message(&response);
        assert!(
            message.contains(&says),
            "case {case}: expected the message to name {says:?}, got {message:?}"
        );
        if let Some(before) = before {
            let after = reported_retention(&emulator, case, case).await;
            assert_eq!(
                after, before,
                "case {case}: a refused change must leave the window at {before}, got {after}"
            );
        }
    }
}

/// A request naming no stream at all is refused by the rule every stream operation applies,
/// before any window is read or written.
#[tokio::test]
async fn a_retention_change_needs_an_identifier() {
    let emulator = Emulator::new();
    for operation in [
        "IncreaseStreamRetentionPeriod",
        "DecreaseStreamRetentionPeriod",
    ] {
        let (status, response) = emulator
            .call(operation, &json!({ "RetentionPeriodHours": 48 }))
            .await;
        assert_eq!(
            status, 400,
            "case {operation}: expected a client error, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidArgumentException",
            "case {operation}: expected InvalidArgumentException, got {response}"
        );
        let message = error_message(&response);
        assert!(
            message.contains(STREAM_IDENTIFIER_REQUIRED_MESSAGE),
            "case {operation}: expected the message to name the identifier rule, got \
             {message:?}"
        );
    }
}
