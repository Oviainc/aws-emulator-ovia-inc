// SPDX-License-Identifier: CC0-1.0

//! The response document this service answers a 200 with, read at the NESTED level.
//!
//! RULE under test: the completeness rule `response_document_completeness.rs` asserts
//! for a response document holds at EVERY DEPTH — a structure the body carries (a
//! summary inside one of the pending lists, the reported `execution`, the reported
//! `executionState`) carries every member ITS shape models, and a member the service
//! has no value for is JSON `null` rather than a key the body leaves out.
//!
//! The two members this service's projection does not send at all
//! (`crate::state::to_job_execution`: `thingName`, and `statusDetails` while the
//! execution carries no name/value pair) stay ABSENT, which is what the recorded wire
//! form of these device-facing operations shows; every other member is reported.
//!
//! Every assertion is made against the body's own KEY SET — `serde_json`'s indexing
//! answers `Null` for a key that is not there, so a missing member and a member
//! reported as `null` are only told apart by looking the key up — and every request
//! goes over the wire (restJson1 through the multi-service router).
//!
//! The rows span what makes a member have no value: an execution that has not started,
//! an execution with no timer armed, a read that did not ask for the job document, an
//! execution carrying no status details — crossed with the same case where the member
//! DOES have a value, so no row's expectation is the trivially empty one.

use serde_json::{json, Value};

// The emulator and its HTTP calls are the ones every suite of this crate shares.
use crate::support::Emulator;

/// The modelled members of each nested shape, from
/// `smithy_query(query="shape",shape="JobExecutionSummary,JobExecution,JobExecutionState")`
/// on this service's model — NOT a hand-picked subset.
const SUMMARY_MEMBERS: &[&str] = &[
    "jobId",
    "queuedAt",
    "startedAt",
    "lastUpdatedAt",
    "versionNumber",
    "executionNumber",
];
const EXECUTION_MEMBERS: &[&str] = &[
    "jobId",
    "thingName",
    "status",
    "statusDetails",
    "queuedAt",
    "startedAt",
    "lastUpdatedAt",
    "approximateSecondsBeforeTimedOut",
    "versionNumber",
    "executionNumber",
    "jobDocument",
];
const EXECUTION_STATE_MEMBERS: &[&str] = &["status", "statusDetails", "versionNumber"];

/// The members of a reported `JobExecution` this service does not send at all while it
/// has nothing to report for them, as `crate::state::to_job_execution` documents.
const EXECUTION_UNSENT: &[&str] = &["thingName", "statusDetails"];

/// The same for a reported `JobExecutionState` (`crate::state::to_execution_state`).
const EXECUTION_STATE_UNSENT: &[&str] = &["statusDetails"];

/// The members every reported summary has a value for, whatever state the execution is
/// in: its id, the stamps the model gives a default of `0`, and the two numbers the
/// store always holds.
const SUMMARY_ALWAYS_VALUED: &[&str] = &[
    "jobId",
    "queuedAt",
    "lastUpdatedAt",
    "versionNumber",
    "executionNumber",
];

/// The same for a reported execution, which additionally always carries its `status`.
const EXECUTION_ALWAYS_VALUED: &[&str] = &[
    "jobId",
    "status",
    "queuedAt",
    "lastUpdatedAt",
    "versionNumber",
    "executionNumber",
];

/// Assert the rule for one nested structure: it is a JSON object whose key set is
/// exactly the members its shape models, apart from the ones in `absent` this service
/// does not send; each member in `valued` carries a value and every other one is JSON
/// `null`.
fn assert_complete_nested(
    case: &str,
    what: &str,
    nested: &Value,
    members: &[&str],
    valued: &[&str],
    absent: &[&str],
) {
    let object = nested.as_object().unwrap_or_else(|| {
        panic!("case {case:?}: {what} must be a JSON object, got {nested}")
    });
    for member in members {
        let found = object.get(*member);
        if absent.contains(member) {
            assert!(
                found.is_none(),
                "case {case:?}: {what} does not send {member:?} at all, got {found:?}"
            );
            continue;
        }
        let Some(found) = found else {
            panic!(
                "case {case:?}: {what} must carry the modelled member {member:?} — as JSON null \
                 when it has no value — got {nested}"
            )
        };
        if valued.contains(member) {
            assert!(
                !found.is_null(),
                "case {case:?}: {what} must report a value for {member:?}, got {found:?}"
            );
        } else {
            assert!(
                found.is_null(),
                "case {case:?}: {what} has no value for {member:?}, so it must be reported as \
                 null, got {found:?}"
            );
        }
    }
    for key in object.keys() {
        assert!(
            members.contains(&key.as_str()),
            "case {case:?}: {what} carries {key:?}, which its shape does not model, got {nested}"
        );
    }
}

/// RULE: every `JobExecutionSummary` in either pending list of
/// `GetPendingJobExecutions` carries every member the shape models — so a queued
/// execution, which has not started, reports `startedAt` as JSON `null` instead of
/// leaving the key out, while a started one reports the instant it started.
///
/// The rows differ in how many executions the thing holds, whether one of them has been
/// started (which decides WHICH list carries it as well as whether `startedAt` has a
/// value) and whether the command's timer was armed at creation.
#[tokio::test]
async fn every_pending_summary_carries_every_modelled_member() {
    let emulator = Emulator::new();

    // (case, thing, how many executions to queue, whether to start the oldest,
    //  the `executionTimeoutSeconds` each is created with)
    let rows: &[(&str, &str, usize, bool, Option<i64>)] = &[
        ("one queued execution", "laser-sum-queued", 1, false, None),
        ("one started execution", "laser-sum-started", 1, true, None),
        ("three queued executions", "laser-sum-three", 3, false, None),
        ("one started beside two queued", "laser-sum-mixed", 3, true, None),
        (
            "a queued execution whose command timer is armed",
            "laser-sum-timer",
            1,
            false,
            Some(3600),
        ),
    ];

    for (case, thing, queued, start_oldest, timeout_seconds) in rows {
        for _ in 0..*queued {
            emulator.start_command_on(thing, *timeout_seconds).await;
        }
        if *start_oldest {
            let (status, body) = emulator.start_next(thing, json!({})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 while arranging the case, got {status} {body}"
            );
        }

        let (status, body) = emulator.pending(thing).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetPendingJobExecutions, got {status} {body}"
        );

        // The started execution is the only one in progress; the rest stay queued.
        let expected_in_progress = usize::from(*start_oldest);
        let expected_queued = queued - expected_in_progress;
        for (list, started, expected_len) in [
            ("inProgressJobs", true, expected_in_progress),
            ("queuedJobs", false, expected_queued),
        ] {
            let summaries = body[list]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: {list} must be a list, got {body}"));
            assert_eq!(
                summaries.len(),
                expected_len,
                "case {case:?}: expected {expected_len} summaries in {list}, got {body}"
            );
            let mut valued: Vec<&str> = SUMMARY_ALWAYS_VALUED.to_vec();
            if started {
                valued.push("startedAt");
            }
            for summary in summaries {
                assert_complete_nested(
                    case,
                    &format!("a summary of {list}"),
                    summary,
                    SUMMARY_MEMBERS,
                    &valued,
                    &[],
                );
            }
        }
    }
}

/// How a row reads the execution it arranged.
enum Read {
    /// `GET /things/{thing}/jobs/{jobId}` with this query string.
    Describe(&'static str),
    /// `PUT /things/{thing}/jobs/$next` with this body.
    Next(Value),
}

/// One row of the reported-execution table.
struct Row {
    case: &'static str,
    thing: &'static str,
    /// The `executionTimeoutSeconds` the command execution is created with, which arms
    /// the timer `approximateSecondsBeforeTimedOut` reports against.
    timeout_seconds: Option<i64>,
    /// Whether the execution is moved to `IN_PROGRESS` before the row's own call.
    started_first: bool,
    read: Read,
    /// The members this row's answer reports a VALUE for, beyond
    /// [`EXECUTION_ALWAYS_VALUED`]. Every other modelled member must be JSON `null`,
    /// except the ones this service does not send.
    also_valued: &'static [&'static str],
}

/// RULE: the `JobExecution` a 200 of `DescribeJobExecution` or
/// `StartNextPendingJobExecution` reports carries every member the shape models —
/// `startedAt` for an execution that has not started,
/// `approximateSecondsBeforeTimedOut` for one with no timer armed and `jobDocument` for
/// a read that did not ask for it are JSON `null`, not missing keys — apart from the
/// members this service does not send at all.
///
/// The rows span which of those members has a value: the execution's status (queued,
/// started by this very call, already in progress), whether a timer is armed (the
/// command's at creation, the device's step timer at the start, none), whether the read
/// asked for the job document, and whether the execution carries status details.
#[tokio::test]
async fn the_reported_execution_carries_every_modelled_member() {
    let emulator = Emulator::new();

    let rows = vec![
        Row {
            case: "a queued execution read without the job document",
            thing: "laser-exec-queued",
            timeout_seconds: None,
            started_first: false,
            read: Read::Describe("?includeJobDocument=false"),
            also_valued: &[],
        },
        Row {
            case: "a queued execution read with the job document the default carries",
            thing: "laser-exec-document",
            timeout_seconds: None,
            started_first: false,
            read: Read::Describe(""),
            also_valued: &["jobDocument"],
        },
        Row {
            case: "a queued execution whose command timer is armed",
            thing: "laser-exec-timer",
            timeout_seconds: Some(3600),
            started_first: false,
            read: Read::Describe("?includeJobDocument=false"),
            also_valued: &["approximateSecondsBeforeTimedOut"],
        },
        Row {
            case: "an execution started by the poll, with no step timer",
            thing: "laser-exec-next",
            timeout_seconds: None,
            started_first: false,
            read: Read::Next(json!({})),
            also_valued: &["startedAt", "jobDocument"],
        },
        Row {
            case: "an execution started by the poll with a step timer",
            thing: "laser-exec-next-timer",
            timeout_seconds: None,
            started_first: false,
            read: Read::Next(json!({"stepTimeoutInMinutes": 5})),
            also_valued: &[
                "startedAt",
                "approximateSecondsBeforeTimedOut",
                "jobDocument",
            ],
        },
        Row {
            case: "an execution started by the poll carrying status details",
            thing: "laser-exec-details",
            timeout_seconds: None,
            started_first: false,
            read: Read::Next(json!({"statusDetails": {"step": "flash"}})),
            also_valued: &["startedAt", "statusDetails", "jobDocument"],
        },
        Row {
            case: "an execution already in progress, read without the job document",
            thing: "laser-exec-in-progress",
            timeout_seconds: None,
            started_first: true,
            read: Read::Describe("?includeJobDocument=false"),
            also_valued: &["startedAt"],
        },
        Row {
            case: "an execution already in progress, re-handed by the poll",
            thing: "laser-exec-rehanded",
            timeout_seconds: Some(3600),
            started_first: true,
            read: Read::Next(json!({})),
            also_valued: &[
                "startedAt",
                "approximateSecondsBeforeTimedOut",
                "jobDocument",
            ],
        },
    ];

    for row in &rows {
        let case = row.case;
        let job_id = emulator
            .start_command_on(row.thing, row.timeout_seconds)
            .await;
        if row.started_first {
            let (status, body) = emulator.start_next(row.thing, json!({})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 while arranging the case, got {status} {body}"
            );
        }

        let (status, body) = match &row.read {
            Read::Describe(query) => emulator.describe(row.thing, &job_id, query).await,
            Read::Next(request) => emulator.start_next(row.thing, request.clone()).await,
        };
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the read, got {status} {body}"
        );

        let mut valued: Vec<&str> = EXECUTION_ALWAYS_VALUED.to_vec();
        valued.extend_from_slice(row.also_valued);
        // A member this service does not normally send IS sent once it has something
        // to report, so the row's own expectation decides.
        let absent: Vec<&str> = EXECUTION_UNSENT
            .iter()
            .copied()
            .filter(|member| !row.also_valued.contains(member))
            .collect();
        assert_complete_nested(
            case,
            "the reported execution",
            &body["execution"],
            EXECUTION_MEMBERS,
            &valued,
            &absent,
        );
    }
}

/// RULE: the `JobExecutionState` a 200 of `UpdateJobExecution` reports when the request
/// asked for it carries every member the shape models, with `statusDetails` left out
/// only while the execution carries no name/value pair.
///
/// The rows span the status the update reports (a pending move, a terminal one) and
/// whether the execution holds status details by then.
#[tokio::test]
async fn the_reported_execution_state_carries_every_modelled_member() {
    let emulator = Emulator::new();

    // (case, the status the update reports, the details it writes, the members the
    //  answer reports a value for beyond `status` and `versionNumber`)
    let rows: &[(&str, &str, Option<&str>, &[&str])] = &[
        ("a pending update with no details", "IN_PROGRESS", None, &[]),
        (
            "a pending update that writes details",
            "IN_PROGRESS",
            Some("flash"),
            &["statusDetails"],
        ),
        ("a terminal update with no details", "SUCCEEDED", None, &[]),
        (
            "a terminal update that writes details",
            "FAILED",
            Some("over-temperature"),
            &["statusDetails"],
        ),
    ];

    for (index, (case, reported_status, detail, also_valued)) in rows.iter().enumerate() {
        // One fresh thing per row, so no row depends on another's execution.
        let thing = format!("laser-state-{index}");
        let job_id = emulator.start_command_on(&thing, None).await;

        let mut request = json!({"status": reported_status, "includeJobExecutionState": true});
        if let Some(detail) = detail {
            request["statusDetails"] = json!({"step": detail});
        }
        let (status, body) = emulator.update(&thing, &job_id, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdateJobExecution, got {status} {body}"
        );

        let mut valued: Vec<&str> = vec!["status", "versionNumber"];
        valued.extend_from_slice(also_valued);
        let absent: Vec<&str> = EXECUTION_STATE_UNSENT
            .iter()
            .copied()
            .filter(|member| !also_valued.contains(member))
            .collect();
        assert_complete_nested(
            case,
            "the reported execution state",
            &body["executionState"],
            EXECUTION_STATE_MEMBERS,
            &valued,
            &absent,
        );
    }
}
