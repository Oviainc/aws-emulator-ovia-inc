// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a Map Run's FAILURE BUDGET and for the request that carries
//! no update at all, driven over this service's own wire protocol and read back
//! through `DescribeMapRun`.
//!
//! The rules under test:
//!
//! 1. a Map Run reports what its items have done SO FAR: an item is counted as
//!    pending until the instant its own iteration finished and under the status that
//!    iteration reached from then on, so the counts grow as the clock reaches them;
//! 2. the thresholds a Map Run carries are a failure BUDGET weighed against the
//!    failures counted so far — the thresholds it carries at that instant, which
//!    `UpdateMapRun` writes — so a Map Run whose counted failures are inside the
//!    budget keeps RUNNING over them and stops running at the failure that BREACHES
//!    it, rather than at the first failed item or only once every item has finished;
//! 3. `UpdateMapRun` changes the settings a request NAMES, and a request that names
//!    none of the three it can change carries no update at all, so it is refused with
//!    the modelled `ValidationException` instead of answered as an update.
//!
//! The tables span how many items fail and when, which threshold states the budget
//! (count, percentage, neither), whether the budget was declared by the `Map` state
//! or written by an update, and — for rule 3 — every subset of the three settings a
//! request can carry.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `string`,
//! `error_code`) is the one `state_machine_lifecycle` already builds; `tests/all.rs`
//! is this binary's crate root, so it is reached through
//! `crate::state_machine_lifecycle`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::state_machine_lifecycle::{
    create_machine, describe_execution, error_code, start_execution, string, Emulator,
};

/// How long the FIRST item of every row below takes, and how long the second one
/// takes after it: the `Map` states run one item at a time, so the two items finish
/// at these two distinct offsets into the execution.
const FIRST_ITEM_SECONDS: u64 = 2;
const SECOND_ITEM_SECONDS: u64 = 3;

/// When each item has been counted, with room for the store's own settle delay and
/// for a loaded machine on either side: the first item finished 2s in and the second
/// 5s in, so these two instants are each comfortably past one and short of the next.
const AFTER_THE_FIRST_ITEM: Duration = Duration::from_millis(4_000);
const AFTER_THE_SECOND_ITEM: Duration = Duration::from_millis(7_000);

/// A Distributed Map over `items`, one item at a time, with the tolerated-failure
/// members `declared` adds.
///
/// Each iteration waits the item's own `delay` seconds and then fails the items whose
/// `ok` is `false`, so the items of one Map Run finish at offsets of their own.
fn distributed_map(items: Value, declared: Value) -> Value {
    let Value::Object(declared) = declared else {
        panic!("expected an object of Map members, got {declared:?}")
    };
    let mut state = json!({
        "Type": "Map",
        "Items": items,
        "MaxConcurrency": 1,
        "ItemProcessor": {
            "ProcessorConfig": { "Mode": "DISTRIBUTED", "ExecutionType": "STANDARD" },
            "StartAt": "W",
            "States": {
                "W": { "Type": "Wait", "SecondsPath": "$.delay", "Next": "C" },
                "C": {
                    "Type": "Choice",
                    "Choices": [{ "Variable": "$.ok", "BooleanEquals": false, "Next": "F" }],
                    "Default": "P",
                },
                "P": { "Type": "Pass", "End": true },
                "F": { "Type": "Fail", "Error": "ProbeError" },
            },
        },
        "End": true,
    });
    for (member, value) in declared {
        state[member] = value;
    }
    state
}

/// Two items that finish at the two offsets above, each failing or not as given.
fn two_items(first_ok: bool, second_ok: bool) -> Value {
    json!([
        { "ok": first_ok, "delay": FIRST_ITEM_SECONDS },
        { "ok": second_ok, "delay": SECOND_ITEM_SECONDS },
    ])
}

/// One state machine definition, as the text `CreateStateMachine` takes.
fn machine_definition(state: Value) -> String {
    json!({ "StartAt": "M", "States": { "M": state } }).to_string()
}

/// Start one execution of a definition and answer `(executionArn, mapRunArn)` of the
/// Map Run its Distributed Map state started.
async fn running_map_run(emulator: &Emulator, name: &str, state: Value) -> (String, String) {
    let machine = create_machine(emulator, name, &machine_definition(state)).await;
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
        "case {name:?}: one Distributed Map state started one Map Run, got {listed:?}"
    );
    (execution, string(&map_runs[0], "mapRunArn"))
}

/// The Map Run record, as `DescribeMapRun` answers for it.
async fn describe(emulator: &Emulator, map_run_arn: &str) -> Value {
    emulator
        .ok("DescribeMapRun", &json!({ "mapRunArn": map_run_arn }))
        .await
}

/// One item count of a described Map Run.
fn count(described: &Value, name: &str) -> i64 {
    described
        .get("itemCounts")
        .and_then(|counts| counts.get(name))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("expected an itemCounts.{name} in {described:?}"))
}

/// What a Map Run reports at one instant: its status and the counts of its items.
fn observed(described: &Value) -> (String, i64, i64, i64) {
    (
        string(described, "status"),
        count(described, "succeeded"),
        count(described, "failed"),
        count(described, "pending"),
    )
}

/// Block until `offset` after `baseline`, which is how every row of one table reaches
/// the same point of its own Map Run's timeline.
fn wait_until(baseline: Instant, offset: Duration) {
    if let Some(remaining) = offset.checked_sub(baseline.elapsed()) {
        std::thread::sleep(remaining);
    }
}

// ---------------------------------------------------------------------------
// Rules 1 and 2: the items are counted as they finish, and the budget decides
// when the Map Run stops running
// ---------------------------------------------------------------------------

/// One row of the budget table.
struct BudgetCase {
    case: &'static str,
    /// The items the `Map` state iterates, the first of which finishes first.
    items: Value,
    /// The tolerated-failure members the `Map` state declares.
    declared: Value,
    /// The members an `UpdateMapRun` carries while the Map Run is still running;
    /// `None` leaves it running under what its state declared.
    update: Option<Value>,
    /// `(status, succeeded, failed, pending)` once the FIRST item has been counted.
    after_first: (&'static str, i64, i64, i64),
    /// The same once the second has been.
    after_second: (&'static str, i64, i64, i64),
    /// What the execution around the Map Run reports at the end.
    execution_status: &'static str,
}

/// A Map Run counts its items as the clock finishes them, keeps RUNNING while the
/// failures counted so far are inside the failure budget it is carrying, and stops
/// running at the failure that breaches that budget.
///
/// The rows span which of the two items fails, whether the budget is a count or a
/// percentage, whether the `Map` state declared it or an `UpdateMapRun` wrote it, and
/// whether the failures end up inside it or past it.
#[tokio::test]
async fn a_map_run_counts_its_items_as_they_finish_and_fails_on_the_breaching_failure() {
    let rows = [
        BudgetCase {
            case: "an update raises the budget to one failure, which the first failure fits",
            items: two_items(false, false),
            declared: json!({}),
            update: Some(json!({ "toleratedFailureCount": 1, "maxConcurrency": 1 })),
            after_first: ("RUNNING", 0, 1, 1),
            after_second: ("FAILED", 0, 2, 0),
            execution_status: "FAILED",
        },
        BudgetCase {
            case: "no budget at all, so the first failure breaches it",
            items: two_items(false, false),
            declared: json!({}),
            update: None,
            after_first: ("FAILED", 0, 1, 1),
            after_second: ("FAILED", 0, 2, 0),
            execution_status: "FAILED",
        },
        BudgetCase {
            case: "a declared count that tolerates both failures",
            items: two_items(false, false),
            declared: json!({ "ToleratedFailureCount": 2 }),
            update: None,
            after_first: ("RUNNING", 0, 1, 1),
            after_second: ("SUCCEEDED", 0, 2, 0),
            execution_status: "SUCCEEDED",
        },
        BudgetCase {
            case: "a declared percentage the first failure fits and the second breaches",
            items: two_items(false, false),
            declared: json!({ "ToleratedFailurePercentage": 50 }),
            update: None,
            after_first: ("RUNNING", 0, 1, 1),
            after_second: ("FAILED", 0, 2, 0),
            execution_status: "FAILED",
        },
        BudgetCase {
            case: "the failing item of two is counted while the one still to come is pending",
            items: two_items(false, true),
            declared: json!({ "ToleratedFailureCount": 1 }),
            update: None,
            after_first: ("RUNNING", 0, 1, 1),
            after_second: ("SUCCEEDED", 1, 1, 0),
            execution_status: "SUCCEEDED",
        },
        BudgetCase {
            case: "every item succeeds, so no budget is ever weighed",
            items: two_items(true, true),
            declared: json!({}),
            update: Some(json!({ "toleratedFailureCount": 0 })),
            after_first: ("RUNNING", 1, 0, 1),
            after_second: ("SUCCEEDED", 2, 0, 0),
            execution_status: "SUCCEEDED",
        },
    ];

    // Every row is started and updated BEFORE anything is waited for: an update is
    // only accepted while the Map Run is in progress, and the rows then share the two
    // instants below because their items take the same time.
    let emulator = Emulator::new();
    let baseline = Instant::now();
    let mut started = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let state = distributed_map(row.items.clone(), row.declared.clone());
        let (execution, arn) =
            running_map_run(&emulator, &format!("budget-{index}"), state).await;
        let case = row.case;
        // Nothing is counted by the call that started the execution: the items ran in
        // the walk, but each is counted from the instant its own iteration finished.
        let described = describe(&emulator, &arn).await;
        let actual = observed(&described);
        assert_eq!(
            actual,
            ("RUNNING".to_string(), 0, 0, 2),
            "case {case:?}: expected a Map Run with both items still pending, got {actual:?} \
             in {described:?}"
        );
        if let Some(update) = row.update.clone() {
            let mut body = update;
            body["mapRunArn"] = Value::String(arn.clone());
            emulator.ok("UpdateMapRun", &body).await;
        }
        started.push((execution, arn));
    }

    wait_until(baseline, AFTER_THE_FIRST_ITEM);
    let mut first_stop_dates = Vec::with_capacity(rows.len());
    for (row, (_, arn)) in rows.iter().zip(&started) {
        let case = row.case;
        let described = describe(&emulator, arn).await;
        let actual = observed(&described);
        let expected = (
            row.after_first.0.to_string(),
            row.after_first.1,
            row.after_first.2,
            row.after_first.3,
        );
        assert_eq!(
            actual, expected,
            "case {case:?}: expected (status, succeeded, failed, pending) {expected:?} once the \
             first item was counted, got {actual:?} in {described:?}"
        );
        first_stop_dates.push(described.get("stopDate").cloned().unwrap_or(Value::Null));
    }

    wait_until(baseline, AFTER_THE_SECOND_ITEM);
    for ((row, (execution, arn)), first_stop_date) in
        rows.iter().zip(&started).zip(&first_stop_dates)
    {
        let case = row.case;
        let described = describe(&emulator, arn).await;
        let actual = observed(&described);
        let expected = (
            row.after_second.0.to_string(),
            row.after_second.1,
            row.after_second.2,
            row.after_second.3,
        );
        assert_eq!(
            actual, expected,
            "case {case:?}: expected (status, succeeded, failed, pending) {expected:?} once the \
             second item was counted, got {actual:?} in {described:?}"
        );
        let stop_date = described.get("stopDate").cloned().unwrap_or(Value::Null);
        assert!(
            !stop_date.is_null(),
            "case {case:?}: a Map Run that has stopped running reports its stopDate, got \
             {described:?}"
        );
        // The instant a Map Run stopped running does not move once it has: a Map Run
        // the budget stopped at the first failure keeps reporting THAT instant while
        // the items behind it are still being counted.
        if !first_stop_date.is_null() {
            assert_eq!(
                &stop_date, first_stop_date,
                "case {case:?}: expected the stopDate to stay at {first_stop_date:?}, got \
                 {stop_date:?} in {described:?}"
            );
        }
        // The execution around the Map Run reports the outcome it reached through the
        // same `Map` state, so the two never contradict each other.
        let execution_status = string(&describe_execution(&emulator, execution).await, "status");
        assert_eq!(
            execution_status, row.execution_status,
            "case {case:?}: expected the execution around the Map Run to report {}, got \
             {execution_status:?}",
            row.execution_status
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a request that names no setting carries no update
// ---------------------------------------------------------------------------

/// One row of the subset table: the settings a request carries beside the ARN, and
/// whether the operation can serve it.
struct SubsetCase {
    case: &'static str,
    update: Value,
    /// The `(status, errorCode)` the request is answered with; a served one is 200.
    served: bool,
}

/// `UpdateMapRun` serves a request that names at least ONE of the three settings it
/// can change, and refuses the one that names none of them: that request carries no
/// update at all, so it is answered with the modelled `ValidationException` (HTTP
/// 400) rather than with a success that changed nothing.
///
/// The rows are every subset of the three settings a request can carry.
#[tokio::test]
async fn update_map_run_refuses_a_request_that_names_no_setting() {
    let rows = [
        SubsetCase {
            case: "no setting at all",
            update: json!({}),
            served: false,
        },
        SubsetCase {
            case: "the max concurrency alone",
            update: json!({ "maxConcurrency": 2 }),
            served: true,
        },
        SubsetCase {
            case: "the tolerated failure count alone",
            update: json!({ "toleratedFailureCount": 3 }),
            served: true,
        },
        SubsetCase {
            case: "the tolerated failure percentage alone",
            update: json!({ "toleratedFailurePercentage": 25 }),
            served: true,
        },
        SubsetCase {
            case: "the max concurrency and the tolerated failure count",
            update: json!({ "maxConcurrency": 2, "toleratedFailureCount": 3 }),
            served: true,
        },
        SubsetCase {
            case: "the max concurrency and the tolerated failure percentage",
            update: json!({ "maxConcurrency": 2, "toleratedFailurePercentage": 25 }),
            served: true,
        },
        SubsetCase {
            case: "both tolerated-failure settings",
            update: json!({ "toleratedFailureCount": 3, "toleratedFailurePercentage": 25 }),
            served: true,
        },
        SubsetCase {
            case: "every setting at once",
            update: json!({
                "maxConcurrency": 2,
                "toleratedFailureCount": 3,
                "toleratedFailurePercentage": 25,
            }),
            served: true,
        },
        SubsetCase {
            // A `0` is a value the three members carry (each is modelled with a
            // minimum of `0`), so a request naming one of them at that value is an
            // update and not an empty request.
            case: "a setting at the minimum of its own range",
            update: json!({ "toleratedFailureCount": 0 }),
            served: true,
        },
    ];

    let emulator = Emulator::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let (_, arn) = running_map_run(
            &emulator,
            &format!("subset-{index}"),
            distributed_map(two_items(true, true), json!({})),
        )
        .await;
        let mut body = row.update.clone();
        body["mapRunArn"] = Value::String(arn.clone());
        let (status, parsed) = emulator.call("UpdateMapRun", &body).await;
        if row.served {
            assert_eq!(
                (status, parsed.as_object().is_some_and(|members| members.is_empty())),
                (200, true),
                "case {case:?}: expected 200 and the empty output this operation models, got \
                 {status} with {parsed:?}"
            );
            continue;
        }
        let code = error_code(&parsed);
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {code} with {parsed:?}"
        );
        assert_eq!(
            parsed.get("message").and_then(Value::as_str),
            Some(
                "Either the max concurrency, the tolerated failure count, or the tolerated \
                 failure percentage must be specified"
            ),
            "case {case:?}: expected the message the service states this rule with, got \
             {parsed:?}"
        );
        assert_eq!(
            parsed.get("reason").and_then(Value::as_str),
            Some("MISSING_REQUIRED_PARAMETER"),
            "case {case:?}: expected the modelled reason for a parameter the request left out, \
             got {parsed:?}"
        );
        // The refusal wrote nothing: the Map Run is still running under what its
        // `Map` state declared.
        let described = describe(&emulator, &arn).await;
        assert_eq!(
            (
                string(&described, "status"),
                described.get("toleratedFailureCount").and_then(Value::as_i64),
            ),
            ("RUNNING".to_string(), Some(0)),
            "case {case:?}: a refused update leaves the Map Run it named alone, got {described:?}"
        );
    }
}
