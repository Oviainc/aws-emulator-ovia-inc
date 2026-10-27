// SPDX-License-Identifier: CC0-1.0

//! The refusal every Lightsail operation that takes a `startTime`/`endTime` pair
//! answers an inverted window with, driven over the wire: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handler and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is the sibling suite's (`instance_lifecycle`), so this
//! file stands up no second emulator of its own.
//!
//! The rule under test: a request whose window `endTime` does not follow its
//! `startTime` is refused with the modelled 400 `InvalidInputException`, and the
//! refusal names each bound as the INSTANT that member carried — its sub-second
//! part and its sign included — so a window inverted inside one second, or one
//! lying before the epoch, reads as the reason for the refusal rather than as two
//! equal whole seconds. Every operation that takes such a pair answers the same
//! sentence.
//!
//! Each window bound a case sends is a modelled `timestamp`, which this protocol
//! carries as (possibly fractional) seconds since the epoch — so the rows differ
//! along the sign, the magnitude and the sub-second part of both bounds, and
//! along how far apart the two instants are: whole seconds, one millisecond, one
//! tenth of a second, and not at all.
//!
//! The ordering of a window is checked before the record the request names is
//! looked up — input a service cannot read is refused before it is resolved — so
//! these rows need no resource to exist. The forward-window control at the end
//! reads the metrics of an instance this suite creates, to show the refusal keys
//! on the ORDER of the two instants rather than on their sub-second part.

use serde_json::{json, Value};

use crate::instance_lifecycle::{error_code, Emulator};

/// The only Region Lightsail serves the distribution API from, from the modelled
/// `InvalidInputException` documentation
/// (`smithy_query(query="doc",shape="InvalidInputException")`): "Domain and
/// distribution APIs are only available in the N. Virginia (us-east-1) Amazon Web
/// Services Region." A distribution request made anywhere else is refused for
/// the Region before its window is read, so the distribution row names it.
const DISTRIBUTION_REGION: &str = "us-east-1";

/// The fixed text of the refusal, either side of the two instants it names.
///
/// Shared with the per-operation suites that pin the rendering of one operation's
/// bounds exactly, so the sentence has one source in this test crate.
pub(crate) const MESSAGE_PREFIX: &str = "startTime must be earlier than endTime, got startTime ";
/// See [`MESSAGE_PREFIX`].
pub(crate) const MESSAGE_INFIX: &str = " and endTime ";
/// See [`MESSAGE_PREFIX`].
pub(crate) const MESSAGE_SUFFIX: &str = " (seconds since the epoch)";

/// The whole sentence an inverted window earns, with each bound rendered as the
/// text the refusal names it by.
pub(crate) fn inverted_window_message(start: &str, end: &str) -> String {
    format!("{MESSAGE_PREFIX}{start}{MESSAGE_INFIX}{end}{MESSAGE_SUFFIX}")
}

/// How far a reported bound may sit from the instant the request named, in
/// seconds.
///
/// A window bound arrives as a JSON number, so it is an `f64` before it is an
/// instant: around the present (~1.7e9 seconds since the epoch) consecutive
/// `f64` values are ~2.4e-7 seconds apart, so a reported bound is the requested
/// one to well inside a microsecond — but not to the nanosecond.
const BOUND_TOLERANCE_SECONDS: f64 = 1e-6;

/// The blueprint, bundle and zone the control instance is created from.
const BLUEPRINT: &str = "amazon_linux_2023";
const BUNDLE: &str = "micro_3_0";
const ZONE: &str = "us-east-1a";

/// One request body carrying the window `start`/`end`, for an operation that
/// takes such a pair.
type WindowRequest = fn(f64, f64) -> Value;

/// Every operation of this service that takes a `startTime`/`endTime` pair, the
/// Region its request has to be made in when its API is served from one Region
/// alone, and a body that is well formed apart from the window.
///
/// SOURCE: the modelled inputs carrying both members, from
/// `smithy_query(query="operation",operation="GetInstanceMetricData,\
/// GetContainerServiceMetricData,GetBucketMetricData,GetDistributionMetricData,\
/// GetContainerLog,GetRelationalDatabaseLogEvents")` — the two log readers model
/// the pair as optional, the four metric readers as `@required`.
const WINDOW_TAKERS: &[(&str, Option<&str>, WindowRequest)] = &[
    ("GetInstanceMetricData", None, instance_metric_request),
    (
        "GetContainerServiceMetricData",
        None,
        container_service_metric_request,
    ),
    ("GetBucketMetricData", None, bucket_metric_request),
    (
        "GetDistributionMetricData",
        Some(DISTRIBUTION_REGION),
        distribution_metric_request,
    ),
    ("GetContainerLog", None, container_log_request),
    (
        "GetRelationalDatabaseLogEvents",
        None,
        relational_database_log_request,
    ),
];

fn instance_metric_request(start: f64, end: f64) -> Value {
    json!({
        "instanceName": "window-probe",
        "metricName": "CPUUtilization",
        "unit": "Percent",
        "period": 300,
        "statistics": ["Average"],
        "startTime": start,
        "endTime": end,
    })
}

fn container_service_metric_request(start: f64, end: f64) -> Value {
    json!({
        "serviceName": "window-probe",
        "metricName": "CPUUtilization",
        "period": 300,
        "statistics": ["Average"],
        "startTime": start,
        "endTime": end,
    })
}

fn bucket_metric_request(start: f64, end: f64) -> Value {
    json!({
        "bucketName": "window-probe",
        "metricName": "NumberOfObjects",
        "unit": "Count",
        "period": 300,
        "statistics": ["Average"],
        "startTime": start,
        "endTime": end,
    })
}

fn distribution_metric_request(start: f64, end: f64) -> Value {
    json!({
        "distributionName": "window-probe",
        "metricName": "Requests",
        "unit": "None",
        "period": 300,
        "statistics": ["Average"],
        "startTime": start,
        "endTime": end,
    })
}

fn container_log_request(start: f64, end: f64) -> Value {
    json!({
        "serviceName": "window-probe",
        "containerName": "app",
        "startTime": start,
        "endTime": end,
    })
}

fn relational_database_log_request(start: f64, end: f64) -> Value {
    json!({
        "relationalDatabaseName": "window-probe",
        "logStreamName": "error",
        "startTime": start,
        "endTime": end,
    })
}

/// A window the ordering rule refuses: the two bounds a case sends, in seconds
/// since the epoch, and whether those two bounds are DIFFERENT instants.
struct InvertedWindow {
    case: &'static str,
    start: f64,
    end: f64,
    distinct: bool,
}

/// The windows every operation above is driven over: inverted by whole seconds,
/// by a tenth of a second, by a millisecond, before the epoch, and the two ways
/// a window can be empty rather than inverted (the same whole second twice, the
/// same sub-second instant twice).
const INVERTED_WINDOWS: &[InvertedWindow] = &[
    InvertedWindow {
        case: "inverted by whole seconds",
        start: 1_700_000_000.0,
        end: 1_699_999_000.0,
        distinct: true,
    },
    InvertedWindow {
        case: "inverted inside one second",
        start: 1_700_000_000.9,
        end: 1_700_000_000.1,
        distinct: true,
    },
    InvertedWindow {
        case: "inverted by one millisecond",
        start: 1_700_000_000.002,
        end: 1_700_000_000.001,
        distinct: true,
    },
    InvertedWindow {
        case: "inverted before the epoch, inside one second",
        start: -0.5,
        end: -0.75,
        distinct: true,
    },
    InvertedWindow {
        case: "inverted before the epoch, by whole seconds",
        start: -100.0,
        end: -200.0,
        distinct: true,
    },
    InvertedWindow {
        case: "the same whole second twice",
        start: 1_700_000_000.0,
        end: 1_700_000_000.0,
        distinct: false,
    },
    InvertedWindow {
        case: "the same sub-second instant twice",
        start: 1_700_000_000.25,
        end: 1_700_000_000.25,
        distinct: false,
    },
];

/// The two instants a refusal names, read back out of its message, which also
/// asserts the sentence's fixed text.
fn reported_bounds(case: &str, message: &str) -> (f64, f64) {
    let rest = message.strip_prefix(MESSAGE_PREFIX).unwrap_or_else(|| {
        panic!("case {case}: expected the message to open with {MESSAGE_PREFIX:?}, got {message:?}")
    });
    let (start, rest) = rest.split_once(MESSAGE_INFIX).unwrap_or_else(|| {
        panic!("case {case}: expected the message to carry {MESSAGE_INFIX:?}, got {message:?}")
    });
    let end = rest.strip_suffix(MESSAGE_SUFFIX).unwrap_or_else(|| {
        panic!("case {case}: expected the message to close with {MESSAGE_SUFFIX:?}, got {message:?}")
    });
    let parse = |member: &str, reported: &str| {
        reported.parse::<f64>().unwrap_or_else(|error| {
            panic!(
                "case {case}: expected {member} to be reported as seconds since the epoch, got \
                 {reported:?} ({error})"
            )
        })
    };
    (parse("startTime", start), parse("endTime", end))
}

// ---------------------------------------------------------------------------
// The refusal, wherever this service takes a window
// ---------------------------------------------------------------------------

/// RULE: a window whose end does not follow its start is refused with the
/// modelled 400 `InvalidInputException`, whose message names each bound as the
/// instant that member carried — sub-second part and sign included — for every
/// operation that takes such a pair.
#[tokio::test]
async fn an_inverted_window_is_refused_naming_the_instants_it_named() {
    let emulator = Emulator::new();

    for (operation, region, build) in WINDOW_TAKERS {
        for window in INVERTED_WINDOWS {
            let case = format!("{operation} over a window {}", window.case);
            let (status, refused) = emulator
                .call_in(*region, operation, build(window.start, window.end))
                .await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} {refused:?}"
            );
            assert_eq!(
                error_code(&refused),
                "InvalidInputException",
                "case {case}: expected the wire error InvalidInputException, got {refused:?}"
            );
            let message = refused["message"].as_str().unwrap_or_else(|| {
                panic!("case {case}: expected a message saying what was wrong, got {refused:?}")
            });
            let (reported_start, reported_end) = reported_bounds(&case, message);

            assert!(
                (reported_start - window.start).abs() <= BOUND_TOLERANCE_SECONDS,
                "case {case}: expected the reported startTime to be the instant the request \
                 named, {}, got {reported_start:?}",
                window.start
            );
            assert!(
                (reported_end - window.end).abs() <= BOUND_TOLERANCE_SECONDS,
                "case {case}: expected the reported endTime to be the instant the request \
                 named, {}, got {reported_end:?}",
                window.end
            );
            if window.distinct {
                assert!(
                    reported_start > reported_end,
                    "case {case}: the message must state the inversion, so the reported \
                     startTime must be later than the reported endTime, got \
                     {reported_start:?} and {reported_end:?}"
                );
            } else {
                assert!(
                    (reported_start - reported_end).abs() <= f64::EPSILON,
                    "case {case}: the request named one instant twice, so both reported bounds \
                     must be it, got {reported_start:?} and {reported_end:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// A window inside one second is still a window
// ---------------------------------------------------------------------------

/// RULE: the refusal keys on the ORDER of the two instants, so a window whose
/// end follows its start by a fraction of a second is served like any other.
#[tokio::test]
async fn a_forward_window_inside_one_second_is_served() {
    let emulator = Emulator::new();
    let instance = "window-control-host";
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [instance],
                "availabilityZone": ZONE,
                "blueprintId": BLUEPRINT,
                "bundleId": BUNDLE,
            }),
        )
        .await;

    // Forward windows differing along how far apart their bounds are: a tenth of
    // a second, a millisecond, and a whole hour, each inside one second of the
    // epoch instant the refused rows above used.
    let windows: &[(&str, f64, f64)] = &[
        ("a tenth of a second", 1_700_000_000.1, 1_700_000_000.9),
        ("one millisecond", 1_700_000_000.001, 1_700_000_000.002),
        ("one hour", 1_700_000_000.5, 1_700_003_600.5),
        ("one hour before the epoch", -3_600.5, -0.5),
    ];

    for (case, start, end) in windows {
        let (status, body) = emulator
            .call(
                "GetInstanceMetricData",
                json!({
                    "instanceName": instance,
                    "metricName": "CPUUtilization",
                    "unit": "Percent",
                    "period": 300,
                    "statistics": ["Average"],
                    "startTime": start,
                    "endTime": end,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case a window of {case}: expected HTTP 200, got {status} {body:?}"
        );
        let data = body["metricData"].as_array().unwrap_or_else(|| {
            panic!("case a window of {case}: expected a metricData array, got {body:?}")
        });
        assert!(
            data.is_empty(),
            "case a window of {case}: this emulator measures nothing on an instance, so the \
             series must carry no datapoints, got {data:?}"
        );
    }
}
