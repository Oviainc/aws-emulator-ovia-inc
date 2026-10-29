// SPDX-License-Identifier: CC0-1.0

//! `DescribeJobExecution$executionNumber` as a SELECTOR on every `jobId` label.
//!
//! The rule under test: `executionNumber` names a particular job execution of the
//! device, so a read that sends it is answered with an execution carrying exactly
//! that number or with nothing at all — on the `$next` label just as on a job's own
//! id. Which label the request used changes only which executions are CANDIDATES
//! (the named job's executions, or the device's pending ones, since `$next` is "the
//! next pending job execution") and which of them is taken when the request named no
//! number; it never makes the member stop selecting. A number no candidate carries is
//! the same 404 `ResourceNotFoundException` the two labels answer every lookup the
//! store cannot resolve with.
//!
//! The rows span what the member can name on a device holding several numbers: the
//! number of the execution `$next` would have taken anyway, the number of another
//! pending one (queued, and the second queued one further down the queue), a number
//! only a FINISHED execution carries (`$next` serves no finished execution, so that
//! is a 404 and not a 410), a number no execution carries at all, and the edges of
//! the `long` the member is modelled as. The named-id rows beside them are the same
//! member doing the same job through the other label.
//!
//! Every row drives the real wire path — restJson1 through the multi-service router.

use serde_json::Value;
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::state::{
    STATUS_IN_PROGRESS, STATUS_QUEUED, STATUS_REJECTED, TERMINAL_STATE_MESSAGE,
};

// The emulator, its HTTP calls and the stored-record helpers are the ones every suite
// of this crate shares.
use crate::support::{error_code, Emulator, STATUS_SUCCEEDED};

/// The `$next` label, as `DescribeJobExecution`'s `jobId` pattern admits it
/// (`^[a-zA-Z0-9_-]+|^\$next$`).
const NEXT: &str = "$next";

/// One execution of a fixture device: the status a device drove it to and the
/// `executionNumber` it carries. One device holds several numbers because a job
/// re-targeted at a device numbers its executions in order, so a read has to be able
/// to name which one it means.
struct Fixture {
    status: &'static str,
    execution_number: i64,
}

/// Start one command execution per fixture row on `thing`, seed each row's status and
/// `executionNumber`, and answer the minted job ids in row order.
async fn seed(emulator: &Emulator, thing: &str, fixture: &[Fixture]) -> Vec<String> {
    let mut ids = Vec::new();
    for _ in fixture {
        ids.push(emulator.start_command_on(thing, None).await);
    }
    for (row, id) in fixture.iter().zip(&ids) {
        emulator.with_stored_execution(thing, id, |record| {
            record.status = row.status.to_string();
            record.started_at = (row.status != STATUS_QUEUED).then_some(record.queued_at);
            record.execution_number = row.execution_number;
        });
    }
    ids
}

/// What a read must answer.
#[derive(Debug, PartialEq, Eq)]
enum Expected {
    /// 200 reporting the fixture execution at this index.
    Reported(usize),
    /// 404 `ResourceNotFoundException`: the request names no execution its label can
    /// reach.
    NotFound,
    /// 410 `TerminalStateException`: the execution the request named has finished.
    Terminal,
}

/// The `executionNumber` a response reports.
fn reported_number(body: &Value) -> Option<i64> {
    body["execution"]["executionNumber"].as_i64()
}

/// RULE: `executionNumber` selects the execution the read reports on both `jobId`
/// labels — a `$next` read that sends it is answered with the pending execution
/// carrying that number, or with 404 when the device has none.
#[tokio::test]
async fn the_execution_number_selects_through_the_next_label_too() {
    let emulator = Emulator::new();
    let thing = "laser-select";
    // One device, four executions: a finished one numbered 1, two queued ones
    // numbered 2 and 5, and a started one numbered 3. With no `executionNumber` at
    // all, `$next` takes the started one (an IN_PROGRESS execution before any
    // QUEUED one).
    let fixture = [
        Fixture {
            status: STATUS_SUCCEEDED,
            execution_number: 1,
        },
        Fixture {
            status: STATUS_QUEUED,
            execution_number: 2,
        },
        Fixture {
            status: STATUS_QUEUED,
            execution_number: 5,
        },
        Fixture {
            status: STATUS_IN_PROGRESS,
            execution_number: 3,
        },
    ];
    let ids = seed(&emulator, thing, &fixture).await;

    // (case, jobId label, query, what the read must answer)
    let rows: Vec<(&str, &str, String, Expected)> = vec![
        (
            "$next with no executionNumber takes the started execution",
            NEXT,
            String::new(),
            Expected::Reported(3),
        ),
        (
            "$next with the started execution's own number",
            NEXT,
            "?executionNumber=3".to_string(),
            Expected::Reported(3),
        ),
        (
            "$next with a queued execution's number",
            NEXT,
            "?executionNumber=2".to_string(),
            Expected::Reported(1),
        ),
        (
            "$next with the number of the queued execution further down the queue",
            NEXT,
            "?executionNumber=5".to_string(),
            Expected::Reported(2),
        ),
        (
            "$next with a number only a finished execution carries",
            NEXT,
            "?executionNumber=1".to_string(),
            Expected::NotFound,
        ),
        (
            "$next with a number no execution of the device carries",
            NEXT,
            "?executionNumber=4".to_string(),
            Expected::NotFound,
        ),
        (
            "$next with zero",
            NEXT,
            "?executionNumber=0".to_string(),
            Expected::NotFound,
        ),
        (
            "$next with the largest number the member's long can carry",
            NEXT,
            format!("?executionNumber={}", i64::MAX),
            Expected::NotFound,
        ),
        (
            "$next with a selected number beside includeJobDocument",
            NEXT,
            "?includeJobDocument=false&executionNumber=2".to_string(),
            Expected::Reported(1),
        ),
        (
            "the queued execution by its own id and its own number",
            &ids[1],
            "?executionNumber=2".to_string(),
            Expected::Reported(1),
        ),
        (
            "the queued execution by its own id and a number it does not carry",
            &ids[1],
            "?executionNumber=5".to_string(),
            Expected::NotFound,
        ),
        (
            "the finished execution by its own id",
            &ids[0],
            String::new(),
            Expected::Terminal,
        ),
        (
            "the finished execution by its own id and its own number",
            &ids[0],
            "?executionNumber=1".to_string(),
            Expected::Terminal,
        ),
    ];

    for (case, label, query, expected) in rows {
        let (status, body) = emulator.describe(thing, label, &query).await;
        match expected {
            Expected::Reported(index) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status} {body}"
                );
                assert_eq!(
                    body["execution"]["jobId"].as_str(),
                    Some(ids[index].as_str()),
                    "case {case:?}: expected the execution {} reported, got {body}",
                    ids[index]
                );
                assert_eq!(
                    reported_number(&body),
                    Some(fixture[index].execution_number),
                    "case {case:?}: expected executionNumber {}, got {:?}",
                    fixture[index].execution_number,
                    reported_number(&body)
                );
                assert_eq!(
                    body["execution"]["status"].as_str(),
                    Some(fixture[index].status),
                    "case {case:?}: expected the status {}, got {body}",
                    fixture[index].status
                );
            }
            Expected::NotFound => {
                assert_eq!(
                    (status, error_code(&body)),
                    (404, "ResourceNotFoundException"),
                    "case {case:?}: expected 404 ResourceNotFoundException, got {status} {body}"
                );
                let expected_message = format!("Job {label} not found for {thing}");
                assert_eq!(
                    body["message"].as_str(),
                    Some(expected_message.as_str()),
                    "case {case:?}: expected the message {expected_message:?}, got {body}"
                );
            }
            Expected::Terminal => assert_eq!(
                (status, error_code(&body), body["message"].as_str()),
                (410, "TerminalStateException", Some(TERMINAL_STATE_MESSAGE)),
                "case {case:?}: expected 410 TerminalStateException, got {status} {body}"
            ),
        }
    }

    // None of those reads moved anything: every fixture execution is still in the
    // status and at the version it was seeded with, and `$next` without a number
    // still takes the started execution.
    for (index, row) in fixture.iter().enumerate() {
        let stored = emulator.stored_execution(thing, &ids[index]);
        assert_eq!(
            (stored.status.as_str(), stored.version_number),
            (row.status, 1),
            "case {:?}: a read must leave the execution alone, got {stored:?}",
            ids[index]
        );
    }
    let (status, body) = emulator.describe(thing, NEXT, "").await;
    assert_eq!(
        (status, body["execution"]["jobId"].as_str()),
        (200, Some(ids[3].as_str())),
        "the unselected $next read must still take the started execution, got {status} {body}"
    );
}

/// RULE as an INVARIANT, swept over the numbers around and past the ones a device
/// holds: a `$next` read either reports an execution whose `executionNumber` is
/// exactly the one the request sent, or reports no execution at all — it is never
/// answered with an execution the request did not name. Which numbers it can be
/// served for is read off the wire (`JobExecutionSummary$executionNumber` of the
/// pending listing) rather than assumed.
#[tokio::test]
async fn next_never_reports_an_execution_number_the_request_did_not_name() {
    let emulator = Emulator::new();
    let thing = "laser:sweep_1";
    // A different shape from the suite's other device: the pending executions are
    // numbered with a gap, and the number between them belongs to an execution the
    // device REJECTED, which is terminal and therefore no candidate for `$next`.
    let fixture = [
        Fixture {
            status: STATUS_QUEUED,
            execution_number: 2,
        },
        Fixture {
            status: STATUS_REJECTED,
            execution_number: 7,
        },
        Fixture {
            status: STATUS_IN_PROGRESS,
            execution_number: 4,
        },
    ];
    seed(&emulator, thing, &fixture).await;

    let (status, pending) = emulator.pending(thing).await;
    assert_eq!(
        status, 200,
        "the pending listing must answer 200, got {status} {pending}"
    );
    let servable: Vec<i64> = ["inProgressJobs", "queuedJobs"]
        .into_iter()
        .filter_map(|list| pending[list].as_array())
        .flatten()
        .filter_map(|summary| summary["executionNumber"].as_i64())
        .collect();
    assert_eq!(
        servable.len(),
        2,
        "the device must hold exactly its two pending executions, got {pending}"
    );

    // One below the lowest number the device holds, through one past the highest,
    // plus the edges of the modelled long.
    let swept: Vec<i64> = (1..=8).chain([i64::MIN, -1, 0, i64::MAX]).collect();
    for number in swept {
        let (status, body) = emulator
            .describe(thing, NEXT, &format!("?executionNumber={number}"))
            .await;
        if servable.contains(&number) {
            assert_eq!(
                status, 200,
                "case executionNumber={number}: the device holds a pending execution with that number, got {status} {body}"
            );
            assert_eq!(
                reported_number(&body),
                Some(number),
                "case executionNumber={number}: expected that number reported, got {:?}",
                reported_number(&body)
            );
        } else {
            assert_eq!(
                (status, error_code(&body)),
                (404, "ResourceNotFoundException"),
                "case executionNumber={number}: the device holds no pending execution with that number, got {status} {body}"
            );
        }
    }
}
