// SPDX-License-Identifier: CC0-1.0

//! The interval rule, end to end over HTTP, for every operation of this service that
//! takes a time range.
//!
//! The rule every case here asserts: a pair of `startTime`/`endTime` bounds names an
//! interval only when the start is STRICTLY before the end, so a reversed pair and a
//! zero-width pair are both refused with HTTP 400 `ValidationException` whose message
//! names the two bounds the request carried — and the refusal happens before any
//! lookup, so it does not depend on the profiling group existing or holding data.
//!
//! Rows vary what the rule generalises over: which operation is called, how far apart
//! the bounds are (a day, a second, a millisecond, nothing at all), their direction,
//! where they sit on the time axis (pre-epoch, the epoch itself, a current instant) and
//! whether the group named exists. The narrowest ACCEPTED interval — bounds a
//! millisecond apart — is a row too, so the check is "strictly increasing" and not "at
//! least a second apart"; the pairs that differ only below the resolution an `f64` of
//! epoch seconds can hold, which pin the comparison as exact, are the sibling table in
//! [`crate::interval_resolution`].
//!
//! The emulator and the HTTP plumbing are the shared [`crate::profiling_group_lifecycle`]
//! harness, not a fork of it.

use aws_smithy_types::DateTime;
use aws_smithy_types::date_time::Format;

use crate::profiling_group_lifecycle::{Answer, Harness};

/// The group every row runs against, created through the API like any other.
const GROUP: &str = "interval-group";

/// A name no group is stored under, so a row can show the interval refusal is answered
/// ahead of the group lookup (which would be a 404).
const MISSING_GROUP: &str = "interval-never-created";

/// One operation of this service that takes a `startTime`/`endTime` range, as the path
/// it is bound to plus the other members a request to it must carry. `pub(crate)` so a
/// sibling suite about the same rule drives the same operation list rather than forking
/// it.
pub(crate) struct Operation {
    pub(crate) name: &'static str,
    /// The operation's path for one profiling group.
    path: fn(&str) -> String,
    /// Members beyond the two bounds that the operation requires (already `&`-prefixed).
    extra: &'static str,
}

/// Every operation whose request names BOTH bounds of a range. `BatchGetFrameMetricData`
/// also models the pair, but is still a scaffolded 501, so it has no behaviour to assert
/// here yet.
pub(crate) fn operations() -> Vec<Operation> {
    vec![
        Operation {
            name: "ListProfileTimes",
            path: |group| format!("/profilingGroups/{group}/profileTimes"),
            // `period` is modelled required for this operation.
            extra: "&period=PT5M",
        },
        Operation {
            name: "ListFindingsReports",
            path: |group| format!("/internal/profilingGroups/{group}/findingsReports"),
            extra: "",
        },
        Operation {
            name: "GetRecommendations",
            path: |group| format!("/internal/profilingGroups/{group}/recommendations"),
            extra: "",
        },
        Operation {
            name: "GetProfile",
            path: |group| format!("/profilingGroups/{group}/profile"),
            extra: "",
        },
    ]
}

/// One pair of bounds, as the ISO 8601 `date-time` text the modelled query members carry,
/// and whether the interval rule accepts it.
#[derive(Debug)]
struct Bounds {
    label: &'static str,
    start: &'static str,
    end: &'static str,
    /// Whether the pair is an interval (start strictly before end).
    is_interval: bool,
}

/// The table. Rows differ in the direction of the pair, how far apart the bounds are
/// (down to a millisecond and down to nothing) and where on the time axis they sit.
fn bounds() -> Vec<Bounds> {
    vec![
        Bounds {
            label: "a pair reversed by a day",
            start: "2026-01-02T00:00:00Z",
            end: "2026-01-01T00:00:00Z",
            is_interval: false,
        },
        Bounds {
            label: "a pair reversed by one second",
            start: "2026-01-01T00:00:01Z",
            end: "2026-01-01T00:00:00Z",
            is_interval: false,
        },
        Bounds {
            label: "a pair reversed by one millisecond",
            start: "2026-01-01T00:00:00.002Z",
            end: "2026-01-01T00:00:00.001Z",
            is_interval: false,
        },
        Bounds {
            label: "equal whole-second bounds",
            start: "2026-01-01T00:00:00Z",
            end: "2026-01-01T00:00:00Z",
            is_interval: false,
        },
        Bounds {
            label: "equal bounds carrying a millisecond fraction",
            start: "2026-01-01T00:00:00.123Z",
            end: "2026-01-01T00:00:00.123Z",
            is_interval: false,
        },
        Bounds {
            label: "equal bounds at the epoch",
            start: "1970-01-01T00:00:00Z",
            end: "1970-01-01T00:00:00Z",
            is_interval: false,
        },
        Bounds {
            label: "equal bounds before the epoch",
            start: "1969-12-31T23:59:59Z",
            end: "1969-12-31T23:59:59Z",
            is_interval: false,
        },
        Bounds {
            label: "a pair increasing by a day",
            start: "2026-01-01T00:00:00Z",
            end: "2026-01-02T00:00:00Z",
            is_interval: true,
        },
        Bounds {
            label: "a pair increasing by one second",
            start: "2026-01-01T00:00:00Z",
            end: "2026-01-01T00:00:01Z",
            is_interval: true,
        },
        Bounds {
            label: "a pair increasing by one millisecond",
            start: "2026-01-01T00:00:00.001Z",
            end: "2026-01-01T00:00:00.002Z",
            is_interval: true,
        },
        Bounds {
            label: "a pair increasing across the epoch",
            start: "1969-12-31T23:59:59Z",
            end: "1970-01-01T00:00:01Z",
            is_interval: true,
        },
    ]
}

/// `at` as the service renders an instant it names in a message: the ISO 8601
/// `date-time` form, which is also the form the request carried it in — so the expected
/// message below is derived from the row's own bounds and not written out by hand.
pub(crate) fn canonical(at: &str) -> String {
    DateTime::from_str(at, Format::DateTime)
        .unwrap_or_else(|error| panic!("unparseable instant {at:?}: {error}"))
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("unformattable instant {at:?}: {error}"))
}

/// The refusal a non-interval must answer with, built from that request's own bounds.
/// `pub(crate)` so every suite about this rule asserts the one sentence.
pub(crate) fn expected_refusal(start: &str, end: &str) -> String {
    format!(
        "Interval start time ({}) must be before end time ({})",
        canonical(start),
        canonical(end)
    )
}

pub(crate) async fn create_group(harness: &Harness, name: &str) {
    let created = harness
        .send(
            "POST",
            &format!("/profilingGroups?clientToken=tok-{name}"),
            "application/json",
            serde_json::to_vec(&serde_json::json!({ "profilingGroupName": name }))
                .expect("a serializable body"),
        )
        .await;
    assert_eq!(
        created.status,
        201,
        "creating group {name} must succeed, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
}

/// Call one ranged operation for one group with the two bounds spelled exactly as the
/// caller gives them. `pub(crate)` so a sibling suite reaches every ranged operation
/// through this one sender.
pub(crate) async fn call_bounds(
    harness: &Harness,
    operation: &Operation,
    group: &str,
    start: &str,
    end: &str,
) -> Answer {
    let uri = format!(
        "{}?startTime={start}&endTime={end}{}",
        (operation.path)(group),
        operation.extra
    );
    harness
        .send("GET", &uri, "application/json", Vec::new())
        .await
}

async fn call(harness: &Harness, operation: &Operation, group: &str, row: &Bounds) -> Answer {
    call_bounds(harness, operation, group, row.start, row.end).await
}

#[tokio::test]
async fn a_pair_of_bounds_that_is_not_an_interval_is_refused_by_every_ranged_operation() {
    let harness = Harness::new();
    create_group(&harness, GROUP).await;
    // One posted profile, so the accepted rows reach a group that actually holds data
    // and the operations have something to report rather than only an absence.
    assert_eq!(
        harness
            .post_agent_profile(GROUP, "application/json", None, b"{\"frames\":[]}".to_vec())
            .await
            .status,
        204,
        "the profile must be stored"
    );

    for operation in operations() {
        for row in bounds() {            let case = format!("{} {:?}", operation.name, row.label);
            let answer = call(&harness, &operation, GROUP, &row).await;

            if row.is_interval {
                assert_ne!(
                    answer.status,
                    400,
                    "case {case}: an interval must not be refused, got {} {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                continue;
            }

            assert_eq!(
                answer.status,
                400,
                "case {case}: expected HTTP 400, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case}: expected ValidationException, got {}",
                String::from_utf8_lossy(&answer.body)
            );
            let expected = expected_refusal(row.start, row.end);
            assert_eq!(
                answer.error_message(),
                expected,
                "case {case}: expected {expected:?}, got {:?}",
                answer.error_message()
            );
        }
    }
}

#[tokio::test]
async fn the_interval_refusal_is_answered_before_the_group_is_looked_up() {
    let harness = Harness::new();

    for operation in operations() {
        for row in bounds().into_iter().filter(|row| !row.is_interval) {
            let case = format!("{} {:?}", operation.name, row.label);
            // No group of this name was ever created, so a lookup would answer 404: the
            // 400 shows the interval is checked first.
            let answer = call(&harness, &operation, MISSING_GROUP, &row).await;
            assert_eq!(
                answer.status,
                400,
                "case {case}: a non-interval must be refused before the group lookup, \
                 got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case}: expected ValidationException, got {}",
                String::from_utf8_lossy(&answer.body)
            );
            let expected = expected_refusal(row.start, row.end);
            assert_eq!(
                answer.error_message(),
                expected,
                "case {case}: expected {expected:?}, got {:?}",
                answer.error_message()
            );
        }
    }
}

#[tokio::test]
async fn a_refused_interval_changes_nothing_a_later_caller_sees() {
    let harness = Harness::new();
    create_group(&harness, GROUP).await;
    assert_eq!(
        harness
            .post_agent_profile(GROUP, "application/json", None, b"{\"frames\":[]}".to_vec())
            .await
            .status,
        204,
        "the profile must be stored"
    );

    // What every ranged operation answers over a window that IS an interval, before any
    // refusal has been attempted.
    let window = Bounds {
        label: "a wide window around the stored profile",
        start: "2000-01-01T00:00:00Z",
        end: "2099-01-01T00:00:00Z",
        is_interval: true,
    };
    let mut before: Vec<(String, u16, Vec<u8>)> = Vec::new();
    for operation in operations() {
        let answer = call(&harness, &operation, GROUP, &window).await;
        before.push((operation.name.to_string(), answer.status, answer.body));
    }

    for operation in operations() {
        for row in bounds().into_iter().filter(|row| !row.is_interval) {
            let answer = call(&harness, &operation, GROUP, &row).await;
            assert_eq!(
                answer.status,
                400,
                "{} {:?}: expected the interval refusal, got {} {}",
                operation.name,
                row.label,
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
        }
    }

    for (operation, (name, status, body)) in operations().into_iter().zip(before) {
        assert_eq!(operation.name, name, "the operations must line up");
        let answer = call(&harness, &operation, GROUP, &window).await;
        assert_eq!(
            answer.status, status,
            "{name}: a refused interval must not change a later answer's status, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.body,
            body,
            "{name}: a refused interval must not change a later answer, got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }
}
