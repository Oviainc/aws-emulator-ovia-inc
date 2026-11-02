// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the WORDS `UpdateMapRun` refuses a Map Run that has already
//! finished in, driven over this service's own wire protocol and read back through
//! `ListMapRuns`/`DescribeMapRun`.
//!
//! The rule under test: a refusal raised because the RESOURCE the request addressed
//! is in a state that does not admit the call states the service's own sentence for
//! that condition AND identifies that resource by the identifier the request named,
//! so a caller changing several of them at once reads which one was refused out of
//! the refusal itself. For `UpdateMapRun` the sentence is `Cannot update completed
//! MapRun <mapRunArn>` — fixed text plus the `mapRunArn` member the request carried.
//!
//! The table spans the class rather than the one request that exposed it: its rows
//! differ along HOW the Map Run reached a terminal status (every item succeeded,
//! every item failed, the execution around it was stopped), along the Map Run ARN the
//! request addresses — the state machine name and the `Map` state's `Label` are both
//! fields of that ARN, and every row varies both — and along which of the three
//! settings the update asks to change. The controls are a Map Run that is still
//! running, which the same request changes, and the record itself, which a refusal
//! leaves exactly where it was.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `string`) is the one
//! `state_machine_lifecycle` builds, `failure` is `error_messages`' own, and the two
//! Distributed Map processors and the `DescribeMapRun` read are `map_run_updates`';
//! `tests/all.rs` is this binary's crate root, so each is reached through `crate::`.

use serde_json::{json, Value};

use crate::error_messages::failure;
use crate::map_run_updates::{describe, ok_or_fail_processor, passing_processor};
use crate::state_machine_lifecycle::{
    create_machine, start_execution, string, Emulator, PAST_THE_EXECUTION_DELAY,
};

/// The `MaxConcurrency` every row's `Map` state declares, so what a refused update
/// did NOT write is visible in the record afterwards.
const DECLARED_CONCURRENCY: i64 = 3;

/// AWS's sentence for a configuration change asked of a Map Run that has already
/// finished, written out here rather than taken from the implementation: the fixed
/// text `Cannot update completed MapRun ` followed by the `mapRunArn` the request
/// carried.
fn expected_refusal(map_run_arn: &str) -> String {
    format!("Cannot update completed MapRun {map_run_arn}")
}

/// How one row's Map Run reaches a terminal status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ending {
    /// Every item succeeds, so the Map Run is `SUCCEEDED` once its items are due.
    EveryItemSucceeds,
    /// Every item fails and the `Map` state tolerates none, so it is `FAILED`.
    EveryItemFails,
    /// The execution around it is stopped while its items still run, so it is
    /// `ABORTED`.
    ExecutionStopped,
}

impl Ending {
    /// The `MapRunStatus` this ending leaves the Map Run at.
    fn status(self) -> &'static str {
        match self {
            Self::EveryItemSucceeds => "SUCCEEDED",
            Self::EveryItemFails => "FAILED",
            Self::ExecutionStopped => "ABORTED",
        }
    }
}

/// One row of the table: how the Map Run ends, the two ARN fields it is addressed
/// by, and the settings the refused update asks to change.
struct Row {
    case: &'static str,
    ending: Ending,
    /// The state machine's name, which the Map Run ARN carries.
    machine: &'static str,
    /// The `Map` state's `Label` — "a user-defined or an auto-generated string that
    /// identifies a Map state" — which the Map Run ARN carries after the name.
    label: &'static str,
    /// The members of `UpdateMapRun` the request names beside `mapRunArn`.
    update: Value,
}

/// A state machine whose one Distributed `Map` state is labelled `label` and ends the
/// way `ending` describes.
fn definition(label: &str, ending: Ending) -> String {
    let (items, processor) = match ending {
        Ending::EveryItemSucceeds => (json!([1, 2]), passing_processor()),
        Ending::EveryItemFails | Ending::ExecutionStopped => {
            (json!([{ "ok": false }, { "ok": false }]), ok_or_fail_processor())
        }
    };
    let mut map = json!({
        "Type": "Map",
        "Label": label,
        "Items": items,
        "ItemProcessor": processor,
        "MaxConcurrency": DECLARED_CONCURRENCY,
    });
    if ending == Ending::ExecutionStopped {
        // The `Wait` keeps the EXECUTION running past the stop, so what is observed
        // is what the stop did to the Map Run itself.
        map["Next"] = json!("W");
        return json!({
            "StartAt": "M",
            "States": { "M": map, "W": { "Type": "Wait", "Seconds": 120, "End": true } },
        })
        .to_string();
    }
    map["End"] = json!(true);
    json!({ "StartAt": "M", "States": { "M": map } }).to_string()
}

/// Start one execution of `row`'s definition and answer the `mapRunArn` of the Map
/// Run its Distributed `Map` state started.
async fn map_run_of(emulator: &Emulator, row: &Row) -> String {
    let machine = create_machine(emulator, row.machine, &definition(row.label, row.ending)).await;
    let execution = start_execution(emulator, &machine, "only", r#"{"go":true}"#).await;
    let listed = emulator
        .ok("ListMapRuns", &json!({ "executionArn": execution }))
        .await;
    let map_runs = listed
        .get("mapRuns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        map_runs.len(),
        1,
        "case {:?}: one Distributed Map state started one Map Run, got {listed:?}",
        row.case
    );
    let arn = string(&map_runs[0], "mapRunArn");
    if row.ending == Ending::ExecutionStopped {
        emulator
            .ok("StopExecution", &json!({ "executionArn": execution }))
            .await;
    }
    arn
}

/// The rows of the table: three endings, each addressed by its own state machine name
/// and `Map` label, and each refused for a different set of requested settings.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "every item succeeded, concurrency alone",
            ending: Ending::EveryItemSucceeds,
            machine: "completed-refusal-succeeded",
            label: "FanOut",
            update: json!({ "maxConcurrency": 1 }),
        },
        Row {
            case: "every item failed, the tolerated count alone",
            ending: Ending::EveryItemFails,
            machine: "completed-refusal-failed",
            label: "Shards",
            update: json!({ "toleratedFailureCount": 99 }),
        },
        Row {
            case: "the execution was stopped, the tolerated percentage alone",
            ending: Ending::ExecutionStopped,
            machine: "completed-refusal-aborted",
            label: "Batches",
            update: json!({ "toleratedFailurePercentage": 50 }),
        },
        Row {
            case: "every item succeeded, all three settings at once",
            ending: Ending::EveryItemSucceeds,
            machine: "completed-refusal-all-three",
            label: "Partitions.v2",
            update: json!({
                "maxConcurrency": 0,
                "toleratedFailureCount": 0,
                "toleratedFailurePercentage": 100,
            }),
        },
    ]
}

/// A Map Run that has already finished is refused with AWS's sentence for that
/// condition, carrying the `mapRunArn` the request addressed it by — and the record
/// is left exactly where its items left it.
#[tokio::test]
async fn the_completed_map_run_refusal_names_the_requested_map_run() {
    let emulator = Emulator::new();

    // Every row's Map Run is started first, so the ones that finish by the CLOCK are
    // all due after one wait rather than one per row.
    let rows = rows();
    let mut started: Vec<(&Row, String)> = Vec::new();
    for row in &rows {
        let arn = map_run_of(&emulator, row).await;
        started.push((row, arn));
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    // No two rows address the same Map Run, so a message that carried a value of the
    // store rather than of the request could not satisfy all of them.
    for (row, arn) in &started {
        assert_eq!(
            started.iter().filter(|(_, other)| other == arn).count(),
            1,
            "case {:?}: each row addresses a Map Run of its own, got {arn:?} twice",
            row.case
        );
    }

    for (row, arn) in &started {
        let described = describe(&emulator, arn).await;
        assert_eq!(
            string(&described, "status"),
            row.ending.status(),
            "case {:?}: expected the Map Run to have ended {:?}, got {described:?}",
            row.case,
            row.ending
        );

        let mut body = row.update.clone();
        body["mapRunArn"] = json!(arn);
        let (status, code, message) = failure(&emulator, row.case, "UpdateMapRun", &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {:?}: expected 400 ValidationException, got {status} {code} with message \
             {message:?}",
            row.case
        );
        let expected = expected_refusal(arn);
        assert_eq!(
            message, expected,
            "case {:?}: expected the message {expected:?}, got {message:?}",
            row.case
        );
        assert!(
            message.contains(arn.as_str()),
            "case {:?}: expected the refusal to name the requested Map Run {arn:?}, got \
             {message:?}",
            row.case
        );

        // The refusal wrote nothing: the settings the `Map` state declared and the
        // status its items left are both still there.
        let after = describe(&emulator, arn).await;
        assert_eq!(
            after.get("maxConcurrency").and_then(Value::as_i64),
            Some(DECLARED_CONCURRENCY),
            "case {:?}: a refused update left the record alone, got {after:?}",
            row.case
        );
        assert_eq!(
            string(&after, "status"),
            row.ending.status(),
            "case {:?}: a refused update left the status alone, got {after:?}",
            row.case
        );
    }
}

/// The control: the SAME request against a Map Run that is still running is served,
/// so what the rows above observe is the terminal status and not the request.
#[tokio::test]
async fn a_running_map_run_takes_the_same_update() {
    let emulator = Emulator::new();
    let row = Row {
        case: "still running",
        ending: Ending::EveryItemSucceeds,
        machine: "completed-refusal-control",
        label: "FanOut",
        update: json!({ "maxConcurrency": 1 }),
    };
    let arn = map_run_of(&emulator, &row).await;

    let mut body = row.update.clone();
    body["mapRunArn"] = json!(&arn);
    emulator.ok("UpdateMapRun", &body).await;

    let described = describe(&emulator, &arn).await;
    assert_eq!(
        described.get("maxConcurrency").and_then(Value::as_i64),
        Some(1),
        "case {:?}: the update the refused rows sent is applied while the Map Run runs, got \
         {described:?}",
        row.case
    );
}
