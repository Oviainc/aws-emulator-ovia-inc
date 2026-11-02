// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateMapRun`, driven over this service's own wire
//! protocol and read back through `ListMapRuns`/`DescribeMapRun`.
//!
//! The rules under test:
//!
//! 1. an update writes the settings its request NAMES onto the in-progress Map Run
//!    its ARN resolves to, and leaves every setting the request does not name at what
//!    the `Map` state declared;
//! 2. an update changes the SETTINGS and not what the iterations produced: the status
//!    the Map Run reaches is the one the thresholds its `Map` state ran its items
//!    under decided — they are tolerated while the items that failed are inside the
//!    count OR inside the percentage of them — and the execution the Map Run ran
//!    inside reports the same outcome, so the two never contradict each other (WHEN a
//!    Map Run stops running does follow the budget it currently carries, which is
//!    `map_run_failure_budget`);
//! 3. a request the operation cannot serve is refused with the modelled error for
//!    what is wrong with it — a defective ARN, a Map Run no record answers for, and a
//!    Map Run that has already finished, which has no in-progress configuration to
//!    change.
//!
//! The tables span those rules along which settings a request carries, the values it
//! carries them with (including each published bound of the ranges the model
//! declares), how the items turned out, and which of the two thresholds decides.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `string`,
//! `error_code`) is the one `state_machine_lifecycle` already builds; `tests/all.rs`
//! is this binary's crate root, so it is reached through
//! `crate::state_machine_lifecycle`.

use serde_json::{json, Value};

use crate::state_machine_lifecycle::{
    create_machine, describe_execution, error_code, start_execution, string, Emulator, ACCOUNT,
    PAST_THE_EXECUTION_DELAY, REGION,
};

/// The `maxConcurrency` a Map Run reports when it is running under NO limit.
///
/// RECORDED value, written out rather than taken from the implementation:
/// `DescribeMapRun` on a `Map` state that declares no `MaxConcurrency` answers
/// `2147483647` (`sfn.describe-map-run-default-fields-and-execution-counts`), which is
/// the largest value the modelled `integer` carries — `MaxConcurrency`'s own default
/// of `0` is what the Amazon States Language defines as no limit.
const UNLIMITED_CONCURRENCY: i64 = 2_147_483_647;

/// The largest `toleratedFailureCount` the modelled `long` carries (range 0..), so a
/// row can probe the member at its own ceiling.
const LARGEST_TOLERATED_COUNT: i64 = i64::MAX;

/// What the `Map` state of [`configured_map`] declares, which every row that does not
/// update a setting must still see.
const DECLARED_CONCURRENCY: i64 = 4;
const DECLARED_TOLERATED_COUNT: i64 = 7;
const DECLARED_TOLERATED_PERCENTAGE: f64 = 25.0;

/// A Distributed Map over `items`, with the configuration and processor given.
fn distributed_map(items: Value, configuration: Value, processor: Value) -> Value {
    let Value::Object(configuration) = configuration else {
        panic!("expected an object of Map members, got {configuration:?}")
    };
    let mut state = json!({
        "Type": "Map",
        "Items": items,
        "ItemProcessor": processor,
        "End": true,
    });
    for (member, value) in configuration {
        state[member] = value;
    }
    state
}

/// A processor whose every iteration succeeds.
///
/// `pub(crate)`, not private: `tests/all.rs` is this binary's crate root, so a
/// sibling suite that needs a Distributed Map's processor reads THIS fixture rather
/// than carrying a second copy of it.
pub(crate) fn passing_processor() -> Value {
    json!({
        "ProcessorConfig": { "Mode": "DISTRIBUTED" },
        "StartAt": "P",
        "States": { "P": { "Type": "Pass", "End": true } },
    })
}

/// A processor that FAILS the items whose `ok` member is `false`.
///
/// `pub(crate)` for the same reason [`passing_processor`] is.
pub(crate) fn ok_or_fail_processor() -> Value {
    json!({
        "ProcessorConfig": { "Mode": "DISTRIBUTED" },
        "StartAt": "C",
        "States": {
            "C": {
                "Type": "Choice",
                "Choices": [{ "Variable": "$.ok", "BooleanEquals": false, "Next": "F" }],
                "Default": "P",
            },
            "P": { "Type": "Pass", "End": true },
            "F": { "Type": "Fail", "Error": "ItemBroke" },
        },
    })
}

/// A Distributed Map state that declares all three of the settings `UpdateMapRun`
/// changes, over two items that both succeed.
fn configured_map() -> Value {
    distributed_map(
        json!([1, 2]),
        json!({
            "MaxConcurrency": DECLARED_CONCURRENCY,
            "ToleratedFailureCount": DECLARED_TOLERATED_COUNT,
            "ToleratedFailurePercentage": DECLARED_TOLERATED_PERCENTAGE,
        }),
        passing_processor(),
    )
}

/// One state machine definition, as the text `CreateStateMachine` takes.
fn machine_definition(state: Value) -> String {
    json!({ "StartAt": "M", "States": { "M": state } }).to_string()
}

/// Start one execution of a definition and answer `(executionArn, mapRunArn)` of the
/// Map Run its Distributed Map state started, which is RUNNING for as long as its
/// items are owed to the clock.
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
///
/// `pub(crate)` for the same reason [`passing_processor`] is.
pub(crate) async fn describe(emulator: &Emulator, map_run_arn: &str) -> Value {
    emulator
        .ok("DescribeMapRun", &json!({ "mapRunArn": map_run_arn }))
        .await
}

/// The three settings `UpdateMapRun` governs, as the record reports them.
fn settings(described: &Value) -> (i64, i64, f64) {
    (
        described
            .get("maxConcurrency")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("expected a maxConcurrency in {described:?}")),
        described
            .get("toleratedFailureCount")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("expected a toleratedFailureCount in {described:?}")),
        described
            .get("toleratedFailurePercentage")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("expected a toleratedFailurePercentage in {described:?}")),
    )
}

// ---------------------------------------------------------------------------
// Rule 1: the settings the request names, and only those
// ---------------------------------------------------------------------------

/// One row of the settings table: the members the request carries, and the
/// `(maxConcurrency, toleratedFailureCount, toleratedFailurePercentage)` the record
/// reports once it is applied.
struct SettingsCase {
    case: &'static str,
    update: Value,
    expected: (i64, i64, f64),
}

/// An update writes the settings it names and leaves the rest where the `Map` state
/// put them, over a table spanning which members a request carries, how many of them
/// it carries at once, and each published bound of the ranges they are modelled with.
#[tokio::test]
async fn update_map_run_writes_the_settings_it_names_and_leaves_the_rest() {
    let declared = (
        DECLARED_CONCURRENCY,
        DECLARED_TOLERATED_COUNT,
        DECLARED_TOLERATED_PERCENTAGE,
    );
    let rows = [
        // A request that names NO setting carries no update at all, so it is refused
        // rather than answered as one: that rule, and every subset of the three
        // settings a request can carry, is `map_run_failure_budget`.
        SettingsCase {
            case: "maxConcurrency alone",
            update: json!({ "maxConcurrency": 1 }),
            expected: (1, declared.1, declared.2),
        },
        SettingsCase {
            case: "maxConcurrency at the published minimum of its range is no limit",
            update: json!({ "maxConcurrency": 0 }),
            expected: (UNLIMITED_CONCURRENCY, declared.1, declared.2),
        },
        SettingsCase {
            case: "maxConcurrency at the largest value the modelled integer carries",
            update: json!({ "maxConcurrency": UNLIMITED_CONCURRENCY }),
            expected: (UNLIMITED_CONCURRENCY, declared.1, declared.2),
        },
        SettingsCase {
            case: "toleratedFailureCount alone",
            update: json!({ "toleratedFailureCount": 3 }),
            expected: (declared.0, 3, declared.2),
        },
        SettingsCase {
            case: "toleratedFailureCount at the published minimum of its range",
            update: json!({ "toleratedFailureCount": 0 }),
            expected: (declared.0, 0, declared.2),
        },
        SettingsCase {
            case: "toleratedFailureCount at the largest value the modelled long carries",
            update: json!({ "toleratedFailureCount": LARGEST_TOLERATED_COUNT }),
            expected: (declared.0, LARGEST_TOLERATED_COUNT, declared.2),
        },
        SettingsCase {
            case: "toleratedFailurePercentage alone",
            update: json!({ "toleratedFailurePercentage": 50 }),
            expected: (declared.0, declared.1, 50.0),
        },
        SettingsCase {
            case: "toleratedFailurePercentage with a fractional part",
            update: json!({ "toleratedFailurePercentage": 12.5 }),
            expected: (declared.0, declared.1, 12.5),
        },
        SettingsCase {
            case: "toleratedFailurePercentage at the published minimum of its range",
            update: json!({ "toleratedFailurePercentage": 0 }),
            expected: (declared.0, declared.1, 0.0),
        },
        SettingsCase {
            case: "toleratedFailurePercentage at the published maximum of its range",
            update: json!({ "toleratedFailurePercentage": 100 }),
            expected: (declared.0, declared.1, 100.0),
        },
        SettingsCase {
            case: "every setting at once",
            update: json!({
                "maxConcurrency": 9,
                "toleratedFailureCount": 2,
                "toleratedFailurePercentage": 75.5,
            }),
            expected: (9, 2, 75.5),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // One emulator and one Map Run per row: each row asserts what ONE update did
        // to the record the `Map` state left.
        let emulator = Emulator::new();
        let case = row.case;
        let (_, arn) =
            running_map_run(&emulator, &format!("settings-{index}"), configured_map()).await;

        let mut body = row.update.clone();
        body["mapRunArn"] = Value::String(arn.clone());
        let updated = emulator.ok("UpdateMapRun", &body).await;
        assert!(
            updated.as_object().is_none_or(|members| members.is_empty()),
            "case {case:?}: UpdateMapRun models no output members, got {updated:?}"
        );

        let described = describe(&emulator, &arn).await;
        let actual = settings(&described);
        assert_eq!(
            actual, row.expected,
            "case {case:?}: expected (maxConcurrency, toleratedFailureCount, \
             toleratedFailurePercentage) {:?}, got {actual:?} in {described:?}",
            row.expected
        );
        // The update changed the CONFIGURATION, not which Map Run this is, nor
        // whether it is still running, nor what its items did.
        assert_eq!(
            (
                string(&described, "mapRunArn"),
                string(&described, "status"),
                described
                    .get("itemCounts")
                    .and_then(|counts| counts.get("total"))
                    .and_then(Value::as_i64)
            ),
            (arn.clone(), "RUNNING".to_string(), Some(2)),
            "case {case:?}: an update leaves the Map Run it updated running over its own \
             items, got {described:?}"
        );
        // A read does not move what it reports: the second describe sees the update
        // and nothing else.
        let again = describe(&emulator, &arn).await;
        let repeated = settings(&again);
        assert_eq!(
            repeated, actual,
            "case {case:?}: two reads of one updated Map Run report one configuration, got \
             {repeated:?} after {actual:?}"
        );
    }
}

/// An update applies to the ONE Map Run its ARN names: the other Map Runs of the same
/// execution keep what their own `Map` states declared.
#[tokio::test]
async fn an_update_reaches_only_the_map_run_its_arn_names() {
    let emulator = Emulator::new();
    let definition = json!({
        "StartAt": "First",
        "States": {
            "First": {
                "Type": "Map",
                "Items": [1, 2],
                "MaxConcurrency": DECLARED_CONCURRENCY,
                "ToleratedFailureCount": DECLARED_TOLERATED_COUNT,
                "ToleratedFailurePercentage": DECLARED_TOLERATED_PERCENTAGE,
                "ItemProcessor": passing_processor(),
                "Next": "Second",
            },
            "Second": {
                "Type": "Map",
                "Items": [1, 2],
                "MaxConcurrency": DECLARED_CONCURRENCY,
                "ToleratedFailureCount": DECLARED_TOLERATED_COUNT,
                "ToleratedFailurePercentage": DECLARED_TOLERATED_PERCENTAGE,
                "ItemProcessor": passing_processor(),
                "End": true,
            },
        },
    })
    .to_string();
    let machine = create_machine(&emulator, "two-map-states", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", r#"{"go":true}"#).await;
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
        2,
        "two Distributed Map states started two Map Runs, got {listed:?}"
    );
    let (first, second) = (
        string(&map_runs[0], "mapRunArn"),
        string(&map_runs[1], "mapRunArn"),
    );

    emulator
        .ok(
            "UpdateMapRun",
            &json!({ "mapRunArn": first, "maxConcurrency": 1, "toleratedFailureCount": 0 }),
        )
        .await;

    let updated = settings(&describe(&emulator, &first).await);
    let untouched = settings(&describe(&emulator, &second).await);
    assert_eq!(
        updated,
        (1, 0, DECLARED_TOLERATED_PERCENTAGE),
        "case {:?}: expected the updated Map Run to carry the update, got {updated:?}",
        "the Map Run the request named"
    );
    assert_eq!(
        untouched,
        (
            DECLARED_CONCURRENCY,
            DECLARED_TOLERATED_COUNT,
            DECLARED_TOLERATED_PERCENTAGE
        ),
        "case {:?}: expected the other Map Run to keep what its own state declared, got \
         {untouched:?}",
        "the Map Run the request did not name"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: an update changes the configuration, not what the items produced
// ---------------------------------------------------------------------------

/// One row of the outcome table: the items the `Map` state iterates, the thresholds
/// it declares, the settings the update changes, and the status BOTH the Map Run and
/// the execution around it reach once the clock makes the items due.
struct OutcomeCase {
    case: &'static str,
    /// The items the `Map` state iterates; an item whose `ok` is `false` fails.
    items: Value,
    /// The tolerated-failure members the `Map` state declares.
    declared: Value,
    /// The members `UpdateMapRun` carries.
    update: Value,
    status: &'static str,
    failed: i64,
}

/// An update changes the SETTINGS, not what the iterations produced: the status the
/// Map Run reaches is the one the thresholds its `Map` state ran its items under
/// decided — the items that failed are tolerated while they are inside the count or
/// inside the percentage of them — and the execution around it reports the same
/// outcome, so the two never contradict each other.
///
/// The rows span how many items failed (none, one of two, both), which threshold the
/// state declares, and whether the update tightens it, loosens it or leaves it alone.
#[tokio::test]
async fn an_update_changes_the_settings_and_not_the_outcome_the_items_produced() {
    let one_of_two = json!([{ "ok": true }, { "ok": false }]);
    let two_of_two = json!([{ "ok": false }, { "ok": false }]);
    let none_of_two = json!([{ "ok": true }, { "ok": true }]);
    let rows = [
        OutcomeCase {
            case: "the declared count tolerated the one failure; a loosened count changes nothing",
            items: one_of_two.clone(),
            declared: json!({ "ToleratedFailureCount": 1 }),
            update: json!({ "toleratedFailureCount": 2 }),
            status: "SUCCEEDED",
            failed: 1,
        },
        OutcomeCase {
            case: "a loosened count does not rescue a run its declared thresholds failed",
            items: json!([{ "ok": true }, { "ok": false }]),
            declared: json!({}),
            update: json!({ "toleratedFailureCount": 1 }),
            status: "FAILED",
            failed: 1,
        },
        OutcomeCase {
            case: "a tightened count does not fail a run its declared count tolerated",
            items: one_of_two.clone(),
            declared: json!({ "ToleratedFailureCount": 2 }),
            update: json!({ "toleratedFailureCount": 0 }),
            status: "SUCCEEDED",
            failed: 1,
        },
        OutcomeCase {
            case: "the declared percentage tolerated exactly the share that failed",
            items: one_of_two.clone(),
            declared: json!({ "ToleratedFailurePercentage": 50 }),
            update: json!({ "toleratedFailurePercentage": 25 }),
            status: "SUCCEEDED",
            failed: 1,
        },
        OutcomeCase {
            case: "a declared percentage under the share that failed",
            items: one_of_two.clone(),
            declared: json!({ "ToleratedFailurePercentage": 25 }),
            update: json!({ "toleratedFailurePercentage": 100 }),
            status: "FAILED",
            failed: 1,
        },
        OutcomeCase {
            case: "a declared count that tolerates every item failing",
            items: two_of_two.clone(),
            declared: json!({ "ToleratedFailureCount": 2 }),
            update: json!({ "toleratedFailureCount": 0 }),
            status: "SUCCEEDED",
            failed: 2,
        },
        OutcomeCase {
            case: "no threshold declared at all tolerates no failed item",
            items: two_of_two,
            declared: json!({}),
            update: json!({ "toleratedFailurePercentage": 100 }),
            status: "FAILED",
            failed: 2,
        },
        OutcomeCase {
            case: "a run no item of which failed is at no threshold's mercy",
            items: none_of_two,
            declared: json!({}),
            update: json!({ "toleratedFailureCount": 0, "toleratedFailurePercentage": 0 }),
            status: "SUCCEEDED",
            failed: 0,
        },
        OutcomeCase {
            case: "an update of the concurrency alone leaves the declared threshold deciding",
            items: one_of_two,
            declared: json!({ "ToleratedFailureCount": 1 }),
            update: json!({ "maxConcurrency": 1 }),
            status: "SUCCEEDED",
            failed: 1,
        },
    ];

    // Every row is started and updated BEFORE anything is waited for, because the
    // update is only accepted while the Map Run is in progress.
    let mut started = Vec::with_capacity(rows.len());
    let emulator = Emulator::new();
    for (index, row) in rows.iter().enumerate() {
        let state = distributed_map(
            row.items.clone(),
            row.declared.clone(),
            ok_or_fail_processor(),
        );
        let (execution, arn) =
            running_map_run(&emulator, &format!("outcome-{index}"), state).await;
        let mut body = row.update.clone();
        body["mapRunArn"] = Value::String(arn.clone());
        emulator.ok("UpdateMapRun", &body).await;
        started.push((execution, arn));
    }

    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for (row, (execution, arn)) in rows.iter().zip(&started) {
        let case = row.case;
        let described = describe(&emulator, arn).await;
        let failed = described
            .get("itemCounts")
            .and_then(|counts| counts.get("failed"))
            .and_then(Value::as_i64);
        assert_eq!(
            (string(&described, "status"), failed),
            (row.status.to_string(), Some(row.failed)),
            "case {case:?}: expected status {} with {} failed item(s), got {described:?}",
            row.status,
            row.failed
        );
        assert!(
            described.get("stopDate").is_some_and(|date| !date.is_null()),
            "case {case:?}: a Map Run whose items are due reports its stopDate, got {described:?}"
        );
        // The execution the Map Run ran inside reports the SAME outcome: a Map Run
        // that failed failed the `Map` state it belongs to, and one that succeeded
        // carried its execution on to the end.
        let execution_status = string(&describe_execution(&emulator, execution).await, "status");
        assert_eq!(
            execution_status, row.status,
            "case {case:?}: expected the execution around the Map Run to report {}, got \
             {execution_status:?}",
            row.status
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: refusals
// ---------------------------------------------------------------------------

/// Every request `UpdateMapRun` cannot serve is refused with the modelled error for
/// what is wrong with it, and the Map Run it names is left exactly as it was.
#[tokio::test]
async fn update_map_run_refuses_what_it_cannot_serve() {
    let emulator = Emulator::new();
    let machine = create_machine(
        &emulator,
        "update-refusals",
        &machine_definition(configured_map()),
    )
    .await;
    let execution = start_execution(&emulator, &machine, "only", r#"{"go":true}"#).await;
    let listed = emulator
        .ok("ListMapRuns", &json!({ "executionArn": execution }))
        .await;
    let arn = string(
        &listed
            .get("mapRuns")
            .and_then(Value::as_array)
            .and_then(|runs| runs.first())
            .cloned()
            .unwrap_or_else(|| panic!("expected the execution's Map Run in {listed:?}")),
        "mapRunArn",
    );

    // (case, mapRunArn, expected status, expected error code)
    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "an ARN that is not an ARN at all",
            "not-an-arn".to_string(),
            400,
            "InvalidArn",
        ),
        (
            "another service's ARN",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:f"),
            400,
            "InvalidArn",
        ),
        (
            "a state machine ARN where a Map Run ARN is required",
            machine.clone(),
            400,
            "InvalidArn",
        ),
        (
            "an execution ARN where a Map Run ARN is required",
            execution.clone(),
            400,
            "InvalidArn",
        ),
        (
            "a Map Run ARN of another region",
            arn.replace(REGION, "eu-west-1"),
            400,
            "InvalidArn",
        ),
        (
            "a Map Run ARN with no resource",
            format!("arn:aws:states:{REGION}:{ACCOUNT}:mapRun"),
            400,
            "InvalidArn",
        ),
        (
            "a well-formed Map Run ARN this store never held",
            format!("arn:aws:states:{REGION}:{ACCOUNT}:mapRun:update-refusals/M:never"),
            400,
            "ResourceNotFound",
        ),
    ];

    for (case, map_run_arn, expected_status, expected) in &cases {
        let (status, parsed) = emulator
            .call(
                "UpdateMapRun",
                &json!({ "mapRunArn": map_run_arn, "maxConcurrency": 1 }),
            )
            .await;
        let code = error_code(&parsed);
        assert_eq!(
            (status, code.as_str()),
            (*expected_status, *expected),
            "case {case:?}: expected {expected_status} {expected}, got {status} {code} with \
             {parsed:?}"
        );
    }

    // The refusals above wrote nothing: the Map Run that does exist still carries
    // what its `Map` state declared.
    let described = describe(&emulator, &arn).await;
    let actual = settings(&described);
    assert_eq!(
        actual,
        (
            DECLARED_CONCURRENCY,
            DECLARED_TOLERATED_COUNT,
            DECLARED_TOLERATED_PERCENTAGE
        ),
        "a refused update leaves the record it named alone, got {described:?}"
    );
}

/// A Map Run that has already finished has no in-progress configuration to change:
/// the update is refused with this operation's own `ValidationException`, carrying the
/// reason the model declares for exactly that, and the record keeps its settings.
#[tokio::test]
async fn a_completed_map_run_cannot_be_updated() {
    let emulator = Emulator::new();
    let (_, arn) = running_map_run(&emulator, "update-completed", configured_map()).await;

    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    let finished = describe(&emulator, &arn).await;
    assert_eq!(
        string(&finished, "status"),
        "SUCCEEDED",
        "case {:?}: the Map Run's items are due, so it has finished, got {finished:?}",
        "once the items are due"
    );

    let (status, parsed) = emulator
        .call(
            "UpdateMapRun",
            &json!({ "mapRunArn": arn, "maxConcurrency": 1, "toleratedFailureCount": 99 }),
        )
        .await;
    let code = error_code(&parsed);
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case {:?}: expected 400 ValidationException, got {status} {code} with {parsed:?}",
        "a completed Map Run"
    );
    assert_eq!(
        parsed.get("reason").and_then(Value::as_str),
        Some("CANNOT_UPDATE_COMPLETED_MAP_RUN"),
        "case {:?}: expected the modelled reason for a completed Map Run, got {parsed:?}",
        "a completed Map Run"
    );

    let described = describe(&emulator, &arn).await;
    let actual = settings(&described);
    assert_eq!(
        actual,
        (
            DECLARED_CONCURRENCY,
            DECLARED_TOLERATED_COUNT,
            DECLARED_TOLERATED_PERCENTAGE
        ),
        "a refused update wrote nothing to the completed record, got {described:?}"
    );
    assert_eq!(
        string(&described, "status"),
        "SUCCEEDED",
        "a refused update left the completed Map Run where it was, got {described:?}"
    );
}

/// A Map Run whose execution was STOPPED has finished too, so it is refused the same
/// way: the status it is left at is not re-decided by a threshold a later caller sends.
#[tokio::test]
async fn an_aborted_map_run_cannot_be_updated() {
    let emulator = Emulator::new();
    // The `Wait` keeps the EXECUTION running past the stop, so what is observed is
    // what the stop did to the Map Run.
    let definition = json!({
        "StartAt": "M",
        "States": {
            "M": {
                "Type": "Map",
                "Items": [{ "ok": false }, { "ok": false }],
                "ItemProcessor": ok_or_fail_processor(),
                "Next": "W",
            },
            "W": { "Type": "Wait", "Seconds": 120, "End": true },
        },
    })
    .to_string();
    let machine = create_machine(&emulator, "update-aborted", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", r#"{"go":true}"#).await;
    let listed = emulator
        .ok("ListMapRuns", &json!({ "executionArn": execution }))
        .await;
    let arn = string(
        &listed
            .get("mapRuns")
            .and_then(Value::as_array)
            .and_then(|runs| runs.first())
            .cloned()
            .unwrap_or_else(|| panic!("expected the execution's Map Run in {listed:?}")),
        "mapRunArn",
    );
    emulator
        .ok("StopExecution", &json!({ "executionArn": execution }))
        .await;

    let (status, parsed) = emulator
        .call(
            "UpdateMapRun",
            &json!({ "mapRunArn": arn, "toleratedFailureCount": 2 }),
        )
        .await;
    let code = error_code(&parsed);
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case {:?}: expected 400 ValidationException, got {status} {code} with {parsed:?}",
        "a Map Run whose execution was stopped"
    );
    let described = describe(&emulator, &arn).await;
    assert_eq!(
        string(&described, "status"),
        "ABORTED",
        "case {:?}: the stopped Map Run is still ABORTED, got {described:?}",
        "a Map Run whose execution was stopped"
    );
}
