// SPDX-License-Identifier: CC0-1.0

//! `DescribeJobExecution` read against the clock-driven `TIMED_OUT` transition.
//!
//! The rule under test: a read of a job execution answers from the status the
//! execution's armed timer has ALREADY decided — a timer whose deadline has passed
//! moves the execution to a terminal status by itself, whether or not any call
//! happened in between — so a 200 from this read reports a pending execution
//! together with the time its timer still has (at least one second), and an
//! execution whose deadline has passed is answered with the refusal a finished
//! execution earns instead of being reported as pending with nothing left.
//!
//! What that makes observable, and what every row below asserts:
//!
//! * a reported execution never carries `approximateSecondsBeforeTimedOut` of zero
//!   beside a `QUEUED`/`IN_PROGRESS` status — the self-contradiction a read that
//!   skipped the transition produced;
//! * an execution past its deadline, named by its own `jobId`, is refused 410
//!   `TerminalStateException`, exactly as one a device drove to a terminal status is;
//! * the same execution named through the `$next` label is not the thing's next
//!   PENDING execution any more, so that read is 404 `ResourceNotFoundException`;
//! * the transition is RECORDED in the shared store (the store the IoT control plane
//!   reads the same record from), stamped at the deadline it was owed at and not at
//!   this read, and a read that owes nothing leaves the record untouched;
//! * the reported remaining time comes from the SOONEST armed timer, and is absent
//!   while no timer is armed.
//!
//! Every row drives the real wire path — restJson1 through the multi-service router.
//! The table spans which timer is armed (the step timer, the execution /
//! in-progress timer, both, none), how far the record is from its deadline (an hour
//! past, a second past, exactly due, still to come), the status the execution is in
//! (`QUEUED`, `IN_PROGRESS`, already finished), how the request names the execution
//! (`jobId`, `jobId` plus `executionNumber`, `$next`) and whether it asks for the job
//! document.
//!
//! `clock_driven_timeout_is_materialised.rs` asserts the same rule across the OTHER
//! operations of this plane (the pending listing, the update, the `$next` poll) and
//! has one describe row; what is here is the read's own surface: which remaining
//! time it reports, how the `jobId` label and `executionNumber` resolve the record
//! the clock decided about, and what `includeJobDocument` carries.

use serde_json::Value;
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::state::{
    now_epoch_seconds, FIRST_EXECUTION_NUMBER, STATUS_IN_PROGRESS, STATUS_QUEUED,
    TERMINAL_STATE_MESSAGE,
};
use service_common::iot_job_executions::STATUS_TIMED_OUT;

// The emulator, the HTTP calls and the stored-record helpers these rows drive are
// the ones every suite of this crate shares.
use crate::support::{error_code, Emulator, STATUS_SUCCEEDED};

/// How much wall clock a row allows to pass between seeding its deadlines and
/// reading the execution back: the reported remaining time is taken at the instant
/// the handler runs, so it may be this many seconds short of the seeded offset. The
/// rows' own offsets are kept well clear of it.
const CLOCK_SLACK_SECONDS: i64 = 5;

/// The `jobDocument` a response carries, or `None` when the read did not report one.
fn job_document(body: &Value) -> Option<&str> {
    body["execution"]["jobDocument"].as_str()
}

/// How the request names the execution it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lookup {
    /// The execution's own `jobId`, with no `executionNumber`: "If not specified,
    /// the latest job execution is returned."
    ById,
    /// The execution's own `jobId` plus the `executionNumber` that names it exactly.
    ByIdAndExecutionNumber,
    /// The `$next` label, which resolves to the thing's next PENDING execution.
    Next,
}

/// One row: the record a device drove to `status` with the deadlines
/// `step_timeout_in` / `execution_timeout_in` seconds from the seeding instant, read
/// the way `lookup` names it.
struct DescribeCase {
    case: &'static str,
    /// The device this row's execution belongs to — one per row, so the rows cannot
    /// interfere with each other.
    thing: &'static str,
    status: &'static str,
    /// `step_timeout_expires_at` as an offset in seconds from the seeding instant; a
    /// negative offset is a deadline that has already passed, `0` one that is
    /// exactly due.
    step_timeout_in: Option<i64>,
    /// `execution_timeout_expires_at` as the same kind of offset: the timer
    /// `StartCommandExecution$executionTimeoutSeconds` and a job's
    /// `inProgressTimeoutInMinutes` both arm.
    execution_timeout_in: Option<i64>,
    lookup: Lookup,
    /// The request's `includeJobDocument`, or `None` for a request that omits it.
    include_job_document: Option<bool>,
    /// The status the clock has already decided for this execution.
    expected_status: &'static str,
}

/// What the read must answer, DERIVED from the status the clock decided and from how
/// the request named the execution.
#[derive(Debug, PartialEq, Eq)]
enum Expected {
    /// A pending execution is reported.
    Reported,
    /// An execution in a terminal status, named by its own id: 410.
    Terminal,
    /// A terminal execution is not the thing's next pending one, so `$next` resolves
    /// to nothing: 404.
    NotFound,
}

impl DescribeCase {
    /// True when the clock owes this row the `TIMED_OUT` transition.
    fn times_out(&self) -> bool {
        self.expected_status == STATUS_TIMED_OUT
    }

    fn is_pending(&self) -> bool {
        self.expected_status == STATUS_QUEUED || self.expected_status == STATUS_IN_PROGRESS
    }

    fn expected(&self) -> Expected {
        match (self.is_pending(), self.lookup) {
            (true, _) => Expected::Reported,
            (false, Lookup::Next) => Expected::NotFound,
            (false, _) => Expected::Terminal,
        }
    }

    /// The deadline the move is owed at: the SOONEST armed timer.
    fn deadline_offset(&self) -> Option<i64> {
        [self.step_timeout_in, self.execution_timeout_in]
            .into_iter()
            .flatten()
            .min()
    }

    /// The path label and query string this row reads with.
    fn request(&self, id: &str) -> (String, String) {
        let label = match self.lookup {
            Lookup::Next => "$next".to_string(),
            _ => id.to_string(),
        };
        let mut query: Vec<String> = Vec::new();
        if let Some(include) = self.include_job_document {
            query.push(format!("includeJobDocument={include}"));
        }
        if self.lookup == Lookup::ByIdAndExecutionNumber {
            query.push(format!("executionNumber={FIRST_EXECUTION_NUMBER}"));
        }
        let query = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        (label, query)
    }
}

/// The table: one row per combination of armed timer, distance from the deadline,
/// stored status, how the request names the execution and whether it asks for the
/// job document.
fn describe_cases() -> Vec<DescribeCase> {
    vec![
        DescribeCase {
            case: "a queued execution's execution timer passed an hour ago, read by id",
            thing: "laser-desc-queued-past",
            status: STATUS_QUEUED,
            step_timeout_in: None,
            execution_timeout_in: Some(-3_600),
            lookup: Lookup::ById,
            include_job_document: None,
            expected_status: STATUS_TIMED_OUT,
        },
        DescribeCase {
            case: "a started execution's step timer passed a second ago, read by id and execution number",
            thing: "laser_desc-step-past",
            status: STATUS_IN_PROGRESS,
            step_timeout_in: Some(-1),
            execution_timeout_in: None,
            lookup: Lookup::ByIdAndExecutionNumber,
            include_job_document: Some(false),
            expected_status: STATUS_TIMED_OUT,
        },
        DescribeCase {
            case: "a queued execution's step timer is exactly due, read through $next",
            thing: "plant:laser:desc:due",
            status: STATUS_QUEUED,
            step_timeout_in: Some(0),
            execution_timeout_in: None,
            lookup: Lookup::Next,
            include_job_document: Some(true),
            expected_status: STATUS_TIMED_OUT,
        },
        DescribeCase {
            case: "both timers armed, the sooner one passed, read by id",
            thing: "laser-desc-both-past",
            status: STATUS_IN_PROGRESS,
            step_timeout_in: Some(-30),
            execution_timeout_in: Some(900),
            lookup: Lookup::ById,
            include_job_document: None,
            expected_status: STATUS_TIMED_OUT,
        },
        DescribeCase {
            case: "both timers armed and still to come, the step timer sooner, read by id",
            thing: "laser-desc-both-future",
            status: STATUS_IN_PROGRESS,
            step_timeout_in: Some(300),
            execution_timeout_in: Some(1_800),
            lookup: Lookup::ById,
            include_job_document: None,
            expected_status: STATUS_IN_PROGRESS,
        },
        DescribeCase {
            case: "a queued execution whose only timer is still to come, read through $next",
            thing: "laser-desc-queued-future",
            status: STATUS_QUEUED,
            step_timeout_in: None,
            execution_timeout_in: Some(3_600),
            lookup: Lookup::Next,
            include_job_document: Some(false),
            expected_status: STATUS_QUEUED,
        },
        DescribeCase {
            case: "no timer is armed at all, read by id and execution number",
            thing: "laser-desc-no-timer",
            status: STATUS_QUEUED,
            step_timeout_in: None,
            execution_timeout_in: None,
            lookup: Lookup::ByIdAndExecutionNumber,
            include_job_document: Some(true),
            expected_status: STATUS_QUEUED,
        },
        DescribeCase {
            case: "an execution a device finished before its deadline, read by id",
            thing: "laser-desc-finished",
            status: STATUS_SUCCEEDED,
            step_timeout_in: Some(-60),
            execution_timeout_in: Some(-60),
            lookup: Lookup::ById,
            include_job_document: None,
            expected_status: STATUS_SUCCEEDED,
        },
        DescribeCase {
            case: "an execution a device finished before its deadline, read through $next",
            thing: "laser-desc-finished-next",
            status: STATUS_SUCCEEDED,
            step_timeout_in: None,
            execution_timeout_in: Some(-5),
            lookup: Lookup::Next,
            include_job_document: None,
            expected_status: STATUS_SUCCEEDED,
        },
    ]
}

/// Start one command execution per row and seed the row's status and deadlines,
/// answering the ids and the instant the deadlines are offset from.
async fn seeded(emulator: &Emulator, cases: &[DescribeCase]) -> (Vec<String>, i64) {
    let mut ids = Vec::new();
    for case in cases {
        ids.push(emulator.start_command_on(case.thing, None).await);
    }
    let seeded_at = now_epoch_seconds();
    for (case, id) in cases.iter().zip(&ids) {
        emulator.with_stored_execution(case.thing, id, |record| {
            record.status = case.status.to_string();
            record.started_at = (case.status != STATUS_QUEUED).then_some(seeded_at - 1_000);
            record.step_timeout_expires_at = case.step_timeout_in.map(|offset| seeded_at + offset);
            record.execution_timeout_expires_at =
                case.execution_timeout_in.map(|offset| seeded_at + offset);
        });
    }
    (ids, seeded_at)
}

/// RULE: the read answers from the status the armed timer has already decided — a
/// pending execution is reported with the time its soonest timer still has, and one
/// whose deadline has passed is refused as finished (404 through `$next`, which
/// serves only pending executions), with the transition recorded in the shared store
/// at the deadline it was owed at.
#[tokio::test]
async fn a_read_answers_from_the_status_the_deadline_already_decided() {
    let emulator = Emulator::new();
    let cases = describe_cases();
    let (ids, seeded_at) = seeded(&emulator, &cases).await;

    for (case, id) in cases.iter().zip(&ids) {
        let label = case.case;
        let before = emulator.stored(case.thing, id);
        let (path_label, query) = case.request(id);
        let (status, body) = emulator.describe(case.thing, &path_label, &query).await;

        match case.expected() {
            Expected::Reported => {
                assert_eq!(
                    (status, body["execution"]["status"].as_str()),
                    (200, Some(case.expected_status)),
                    "case {label:?}: a live execution must be reported as {}, got {status} {body}",
                    case.expected_status
                );
                assert_eq!(
                    body["execution"]["jobId"].as_str(),
                    Some(id.as_str()),
                    "case {label:?}: expected the execution {id} reported, got {body}"
                );
                assert_eq!(
                    body["execution"]["executionNumber"].as_i64(),
                    Some(FIRST_EXECUTION_NUMBER),
                    "case {label:?}: expected executionNumber {FIRST_EXECUTION_NUMBER}, got {body}"
                );

                let remaining = body["execution"]["approximateSecondsBeforeTimedOut"].as_i64();
                match case.deadline_offset() {
                    Some(offset) => {
                        let reported = remaining.unwrap_or_else(|| {
                            panic!("case {label:?}: an armed timer must report its remaining time, got {body}")
                        });
                        // The invariant the clock-driven transition buys: a pending
                        // status is never reported with no time left before the
                        // timeout it is pending against.
                        assert!(
                            reported >= 1,
                            "case {label:?}: a reported pending execution must have time left, got {reported}"
                        );
                        assert!(
                            reported <= offset && reported > offset - CLOCK_SLACK_SECONDS,
                            "case {label:?}: expected about {offset} seconds left before the soonest timer, got {reported}"
                        );
                    }
                    None => assert_eq!(
                        remaining, None,
                        "case {label:?}: no armed timer reports no remaining time, got {body}"
                    ),
                }

                // "Unless set to false, the response contains the job document. The
                // default is true" — a command execution's document is the payload
                // its start stored.
                assert_eq!(
                    job_document(&body).is_some(),
                    case.include_job_document != Some(false),
                    "case {label:?}: includeJobDocument={:?} must decide whether the document is reported, got {body}",
                    case.include_job_document
                );
            }
            Expected::Terminal => assert_eq!(
                (status, error_code(&body), body["message"].as_str()),
                (410, "TerminalStateException", Some(TERMINAL_STATE_MESSAGE)),
                "case {label:?}: a finished execution must be refused as terminal, got {status} {body}"
            ),
            Expected::NotFound => {
                assert_eq!(
                    (status, error_code(&body)),
                    (404, "ResourceNotFoundException"),
                    "case {label:?}: $next must find no pending execution, got {status} {body}"
                );
                assert!(
                    body["message"]
                        .as_str()
                        .is_some_and(|message| message.contains(case.thing)),
                    "case {label:?}: the refusal must name the thing it was made for, got {body}"
                );
            }
        }

        // The status the read answered from is the one the SHARED store holds — the
        // record the IoT control plane reads is the same one — stamped at the
        // deadline the move was owed at, not at this read; a row the clock owes
        // nothing is left exactly as it was.
        let after = emulator.stored(case.thing, id);
        let expected_after = if case.times_out() {
            let deadline = seeded_at
                + case
                    .deadline_offset()
                    .unwrap_or_else(|| panic!("case {label:?}: a timed-out row arms a timer"));
            (STATUS_TIMED_OUT.to_string(), deadline)
        } else {
            before.clone()
        };
        assert_eq!(
            after, expected_after,
            "case {label:?}: expected the store to hold {expected_after:?}, got {after:?}"
        );

        // Reading again changes nothing further: the answer and the record are stable.
        let (status_again, body_again) = emulator.describe(case.thing, &path_label, &query).await;
        assert_eq!(
            status_again, status,
            "case {label:?}: a second read must answer {status}, got {status_again} {body_again}"
        );
        assert_eq!(
            body_again["execution"]["status"].as_str(),
            body["execution"]["status"].as_str(),
            "case {label:?}: a second read must report the same status, got {body_again}"
        );
        assert_eq!(
            emulator.stored(case.thing, id),
            after,
            "case {label:?}: a second read must leave the record alone"
        );
    }
}

/// RULE: the remaining time a read reports is the time the timer the service ARMED
/// still has, so an execution started with `executionTimeoutSeconds` reports that
/// duration down to the instant of the read, and one started without a timeout
/// reports no remaining time at all.
///
/// The rows differ in the duration the start armed (none, a minute, an hour, a day)
/// and in whether the read asks for the job document.
#[tokio::test]
async fn the_time_a_read_reports_is_what_the_armed_timer_still_has() {
    let emulator = Emulator::new();

    // (case, thing, executionTimeoutSeconds, includeJobDocument)
    let rows: [(&str, &str, Option<i64>, Option<bool>); 5] = [
        ("no timeout was asked for", "laser-armed-none", None, None),
        ("a minute", "laser-armed-minute", Some(60), Some(true)),
        ("an hour", "laser-armed-hour", Some(3_600), Some(false)),
        ("a day", "laser-armed-day", Some(86_400), None),
        ("a week", "laser-armed-week", Some(604_800), Some(true)),
    ];

    for (case, thing, timeout_seconds, include_job_document) in rows {
        let id = emulator.start_command_on(thing, timeout_seconds).await;
        let query = include_job_document
            .map(|include| format!("?includeJobDocument={include}"))
            .unwrap_or_default();
        let (status, body) = emulator.describe(thing, &id, &query).await;
        assert_eq!(
            (status, body["execution"]["status"].as_str()),
            (200, Some(STATUS_QUEUED)),
            "case {case:?}: a freshly started execution must be reported as queued, got {status} {body}"
        );

        let reported = body["execution"]["approximateSecondsBeforeTimedOut"].as_i64();
        match timeout_seconds {
            Some(seconds) => {
                let reported = reported.unwrap_or_else(|| {
                    panic!("case {case:?}: an armed timer must report its remaining time, got {body}")
                });
                assert!(
                    reported >= 1 && reported <= seconds && reported > seconds - CLOCK_SLACK_SECONDS,
                    "case {case:?}: expected about {seconds} seconds left, got {reported}"
                );
            }
            None => assert_eq!(
                reported, None,
                "case {case:?}: a start that armed no timer reports no remaining time, got {body}"
            ),
        }

        assert_eq!(
            job_document(&body).is_some(),
            include_job_document != Some(false),
            "case {case:?}: includeJobDocument={include_job_document:?} must decide whether the document is reported, got {body}"
        );
        assert_eq!(
            emulator.stored(thing, &id).0,
            STATUS_QUEUED,
            "case {case:?}: the read must leave the queued status alone"
        );
    }
}
