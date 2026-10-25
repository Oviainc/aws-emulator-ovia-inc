// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListMapRuns` and `DescribeMapRun`, driven over this
//! service's own wire protocol.
//!
//! The rules under test:
//!
//! 1. `ListMapRuns` reports, for the execution its `executionArn` names, one Map Run
//!    per Distributed Map state that execution's run entered — each with its own
//!    `mapRun` ARN built from the state machine's name and the state's label, the
//!    execution and state machine it belongs to, the instant its state was entered,
//!    and a `stopDate` only once its items are no longer owed to the clock — paged by
//!    `maxResults`/`nextToken`; an inline `Map` starts no Map Run, and a request
//!    naming no execution of this endpoint's region is refused with this operation's
//!    own modelled error rather than an empty page;
//! 2. `DescribeMapRun` answers from that same record, resolved by the Map Run's own
//!    ARN: the configuration the `Map` state declared and the counts its items
//!    produced.
//!
//! The tables below span those rules: the number of Distributed Map states, where
//! they sit in the definition (in sequence, nested, inside a `Parallel` branch),
//! whether the state declares a `Label`, what it configures, and how its items
//! turned out.

use serde_json::{json, Value};

use crate::state_machine_lifecycle::{
    create_machine, error_code, start_execution, string, Emulator, ACCOUNT,
    PAST_THE_EXECUTION_DELAY, REGION,
};

/// Two items for every definition below to iterate.
const TWO_ITEMS: &str = r#"{"items":[1,2]}"#;

/// A Distributed Map state over two literal items whose processor is one `Pass`,
/// with the `Label` it declares (if any) and the state it transitions to (`End` when
/// none).
///
/// The items are LITERAL so a row may chain several of these: a `Map` state's output
/// is what its iterations produced, so a second state reading the execution input
/// through `ItemsPath` would not find it.
fn distributed_map(label: Option<&str>, next: Option<&str>) -> Value {
    let mut state = json!({
        "Type": "Map",
        "Items": [1, 2],
        "ItemProcessor": {
            "ProcessorConfig": { "Mode": "DISTRIBUTED" },
            "StartAt": "P",
            "States": { "P": { "Type": "Pass", "End": true } },
        },
    });
    match next {
        Some(next) => state["Next"] = json!(next),
        None => state["End"] = json!(true),
    }
    if let Some(label) = label {
        state["Label"] = json!(label);
    }
    state
}

/// One state machine definition, as the text `CreateStateMachine` takes.
fn machine_definition(start_at: &str, states: Value) -> String {
    json!({ "StartAt": start_at, "States": states }).to_string()
}

/// One row of the Map Run table: a definition, and the labels of the Map Runs the
/// execution that runs it must report, in order.
struct MapRunCase {
    case: &'static str,
    definition: String,
    labels: &'static [&'static str],
}

fn cases() -> Vec<MapRunCase> {
    vec![
        MapRunCase {
            case: "an inline Map starts no Map Run",
            definition: machine_definition(
                "M",
                json!({
                    "M": {
                        "Type": "Map",
                        "ItemsPath": "$.items",
                        "ItemProcessor": {
                            "StartAt": "P",
                            "States": { "P": { "Type": "Pass", "End": true } },
                        },
                        "End": true,
                    },
                }),
            ),
            labels: &[],
        },
        MapRunCase {
            case: "a definition with no Map at all starts no Map Run",
            definition: machine_definition(
                "P",
                json!({ "P": { "Type": "Pass", "End": true } }),
            ),
            labels: &[],
        },
        MapRunCase {
            case: "one Distributed Map, labelled by its own state name",
            definition: machine_definition("M", json!({ "M": distributed_map(None, None) })),
            labels: &["M"],
        },
        MapRunCase {
            case: "a Distributed Map whose items come from the execution input",
            definition: machine_definition(
                "FromInput",
                json!({
                    "FromInput": {
                        "Type": "Map",
                        "ItemsPath": "$.items",
                        "ItemProcessor": {
                            "ProcessorConfig": { "Mode": "DISTRIBUTED" },
                            "StartAt": "P",
                            "States": { "P": { "Type": "Pass", "End": true } },
                        },
                        "End": true,
                    },
                }),
            ),
            labels: &["FromInput"],
        },
        MapRunCase {
            case: "one Distributed Map that declares a Label of its own",
            definition: machine_definition(
                "M",
                json!({ "M": distributed_map(Some("mapStateLabel"), None) }),
            ),
            labels: &["mapStateLabel"],
        },
        MapRunCase {
            case: "two Distributed Maps in sequence, in the order they were entered",
            definition: machine_definition(
                "First",
                json!({
                    "First": distributed_map(None, Some("Second")),
                    "Second": distributed_map(None, None),
                }),
            ),
            labels: &["First", "Second"],
        },
        MapRunCase {
            case: "a Distributed Map nested inside a Distributed Map, outermost first",
            definition: machine_definition(
                "Outer",
                json!({
                    "Outer": {
                        "Type": "Map",
                        "Items": [1, 2],
                        "ItemProcessor": {
                            "ProcessorConfig": { "Mode": "DISTRIBUTED" },
                            "StartAt": "Inner",
                            "States": {
                                "Inner": {
                                    "Type": "Map",
                                    "Items": [1],
                                    "ItemProcessor": {
                                        "ProcessorConfig": { "Mode": "DISTRIBUTED" },
                                        "StartAt": "P",
                                        "States": { "P": { "Type": "Pass", "End": true } },
                                    },
                                    "End": true,
                                },
                            },
                        },
                        "End": true,
                    },
                }),
            ),
            labels: &["Outer", "Inner", "Inner"],
        },
        MapRunCase {
            case: "a Distributed Map inside a Parallel branch",
            definition: machine_definition(
                "Fan",
                json!({
                    "Fan": {
                        "Type": "Parallel",
                        "Branches": [{
                            "StartAt": "InBranch",
                            "States": { "InBranch": distributed_map(None, None) },
                        }],
                        "End": true,
                    },
                }),
            ),
            labels: &["InBranch"],
        },
        MapRunCase {
            case: "a Distributed Map whose items failed past what it tolerates",
            definition: machine_definition(
                "Broken",
                json!({
                    "Broken": {
                        "Type": "Map",
                        "Items": [1, 2],
                        "ItemProcessor": {
                            "ProcessorConfig": { "Mode": "DISTRIBUTED" },
                            "StartAt": "F",
                            "States": { "F": { "Type": "Fail", "Error": "ItemBroke" } },
                        },
                        "End": true,
                    },
                }),
            ),
            labels: &["Broken"],
        },
    ]
}

/// The `mapRuns` of one execution, which must fit in one page.
async fn list_map_runs(emulator: &Emulator, execution_arn: &str) -> Vec<Value> {
    let listed = emulator
        .ok("ListMapRuns", &json!({ "executionArn": execution_arn }))
        .await;
    assert!(
        listed.get("nextToken").is_none_or(Value::is_null),
        "a single page of Map Runs carries no nextToken, got {listed:?}"
    );
    listed
        .get("mapRuns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("expected a mapRuns array in {listed:?}"))
}

/// The `member` of the `group` counts structure of a `DescribeMapRun` body.
fn integer(body: &Value, group: &str, member: &str) -> i64 {
    body.get(group)
        .and_then(|counts| counts.get(member))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("expected an integer '{group}.{member}' in {body:?}"))
}

// ---------------------------------------------------------------------------
// Start -> ListMapRuns: one Map Run per Distributed Map state
// ---------------------------------------------------------------------------

/// Every Distributed Map state an execution entered is reported as a Map Run of that
/// execution, with the ARN, the owning execution and state machine, and the dates the
/// record holds; an inline `Map` and a definition with no `Map` report none.
#[tokio::test]
async fn list_map_runs_reports_the_map_runs_the_execution_started() {
    let emulator = Emulator::new();
    let rows = cases();

    // Every row is started first and read afterwards, so one wait covers the outcomes
    // the clock owed all of them.
    let mut started = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let name = format!("maprun-{index}");
        let machine = create_machine(&emulator, &name, &row.definition).await;
        let execution = start_execution(&emulator, &machine, "only", TWO_ITEMS).await;
        started.push((name, machine, execution));
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for (row, (name, machine, execution)) in rows.iter().zip(&started) {
        let case = row.case;
        let listed = list_map_runs(&emulator, execution).await;
        let labels: Vec<String> = listed
            .iter()
            .map(|map_run| {
                let arn = string(map_run, "mapRunArn");
                let prefix = format!("arn:aws:states:{REGION}:{ACCOUNT}:mapRun:{name}/");
                let rest = arn.strip_prefix(&prefix).unwrap_or_else(|| {
                    panic!("case {case:?}: expected a Map Run ARN under {prefix:?}, got {arn:?}")
                });
                let (label, id) = rest
                    .rsplit_once(':')
                    .unwrap_or_else(|| panic!("case {case:?}: expected a trailing id on {arn:?}"));
                assert!(
                    !id.is_empty(),
                    "case {case:?}: expected a non-empty Map Run id on {arn:?}"
                );
                assert_eq!(
                    string(map_run, "executionArn"),
                    *execution,
                    "case {case:?}: a Map Run names the execution that started it, got {map_run:?}"
                );
                assert_eq!(
                    string(map_run, "stateMachineArn"),
                    *machine,
                    "case {case:?}: a Map Run names the state machine that was executed, got \
                     {map_run:?}"
                );
                assert!(
                    map_run.get("startDate").is_some_and(|date| !date.is_null()),
                    "case {case:?}: a Map Run reports the date it started, got {map_run:?}"
                );
                assert!(
                    map_run.get("stopDate").is_some_and(|date| !date.is_null()),
                    "case {case:?}: a Map Run whose items are due reports a stopDate, got \
                     {map_run:?}"
                );
                label.to_string()
            })
            .collect();
        assert_eq!(
            labels, row.labels,
            "case {case:?}: expected the Map Run labels {:?}, got {labels:?}",
            row.labels
        );
    }

    // Each Map Run has an ARN of its own: no two of the records above collide, which
    // is what lets `DescribeMapRun` resolve one.
    let mut arns: Vec<String> = Vec::new();
    for (_, _, execution) in &started {
        for map_run in list_map_runs(&emulator, execution).await {
            arns.push(string(&map_run, "mapRunArn"));
        }
    }
    let total = arns.len();
    arns.sort();
    arns.dedup();
    assert_eq!(
        arns.len(),
        total,
        "every Map Run carries an ARN of its own, got {arns:?}"
    );
}

/// Two executions of ONE state machine report Map Runs of their own: the record
/// belongs to the execution that started it, not to the state machine.
#[tokio::test]
async fn each_execution_reports_only_its_own_map_runs() {
    let emulator = Emulator::new();
    let definition = machine_definition("M", json!({ "M": distributed_map(None, None) }));
    let machine = create_machine(&emulator, "maprun-per-execution", &definition).await;
    let first = start_execution(&emulator, &machine, "first", TWO_ITEMS).await;
    let second = start_execution(&emulator, &machine, "second", TWO_ITEMS).await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let mine = list_map_runs(&emulator, &first).await;
    let theirs = list_map_runs(&emulator, &second).await;
    assert_eq!(
        (mine.len(), theirs.len()),
        (1, 1),
        "each execution reports exactly its own Map Run, got {mine:?} and {theirs:?}"
    );
    let (mine, theirs) = (
        string(&mine[0], "mapRunArn"),
        string(&theirs[0], "mapRunArn"),
    );
    assert_ne!(
        mine, theirs,
        "two executions of one state machine start two Map Runs, got {mine:?} twice"
    );
}

/// One `Map` state a walk enters more than once starts a Map Run EACH time: a
/// Distributed Map nested inside another runs once per outer item, so each of those
/// entries is a record of its own — not one record reporting a redrive nobody asked
/// for.
#[tokio::test]
async fn a_map_state_entered_more_than_once_starts_a_map_run_each_time() {
    let emulator = Emulator::new();
    let definition = machine_definition(
        "Outer",
        json!({
            "Outer": {
                "Type": "Map",
                "Items": [1, 2, 3],
                "ItemProcessor": {
                    "ProcessorConfig": { "Mode": "DISTRIBUTED" },
                    "StartAt": "Inner",
                    "States": {
                        "Inner": {
                            "Type": "Map",
                            "Items": [1],
                            "ItemProcessor": {
                                "ProcessorConfig": { "Mode": "DISTRIBUTED" },
                                "StartAt": "P",
                                "States": { "P": { "Type": "Pass", "End": true } },
                            },
                            "End": true,
                        },
                    },
                },
                "End": true,
            },
        }),
    );
    let machine = create_machine(&emulator, "maprun-reentered", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", TWO_ITEMS).await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let listed = list_map_runs(&emulator, &execution).await;
    assert_eq!(
        listed.len(),
        4,
        "the outer state was entered once and the inner one once per outer item, got \
         {listed:?}"
    );
    let mut arns: Vec<String> = listed
        .iter()
        .map(|map_run| string(map_run, "mapRunArn"))
        .collect();
    for arn in &arns {
        let described = emulator
            .ok("DescribeMapRun", &json!({ "mapRunArn": arn }))
            .await;
        assert_eq!(
            described.get("redriveCount").and_then(Value::as_i64),
            Some(0),
            "case {arn:?}: nobody redrove this execution, got {described:?}"
        );
        assert!(
            described.get("redriveDate").is_none_or(Value::is_null),
            "case {arn:?}: nobody redrove this execution, got {described:?}"
        );
    }
    let total = arns.len();
    arns.sort();
    arns.dedup();
    assert_eq!(
        arns.len(),
        total,
        "each entry of the inner state is a Map Run of its own, got {arns:?}"
    );
}

// ---------------------------------------------------------------------------
// What the clock owes
// ---------------------------------------------------------------------------

/// A Map Run is RUNNING — so it has no `stopDate` — until the instant its items were
/// due, and the ARN it is reported under does not change when it settles.
#[tokio::test]
async fn a_map_run_has_no_stop_date_until_its_items_are_due() {
    let emulator = Emulator::new();
    let definition = machine_definition("M", json!({ "M": distributed_map(None, None) }));
    let machine = create_machine(&emulator, "maprun-settles", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", TWO_ITEMS).await;

    let running = list_map_runs(&emulator, &execution).await;
    assert_eq!(
        running.len(),
        1,
        "the started execution reports its Map Run straight away, got {running:?}"
    );
    assert!(
        running[0].get("stopDate").is_none_or(Value::is_null),
        "case {:?}: a Map Run whose items are still owed reports no stopDate, got {:?}",
        "straight after the start",
        running[0]
    );

    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    let settled = list_map_runs(&emulator, &execution).await;
    assert_eq!(
        settled.len(),
        1,
        "case {:?}: the same one Map Run is reported, got {settled:?}",
        "once the items are due"
    );
    assert_eq!(
        string(&settled[0], "mapRunArn"),
        string(&running[0], "mapRunArn"),
        "a Map Run keeps its ARN when it settles, got {settled:?} after {running:?}"
    );
    assert!(
        settled[0].get("stopDate").is_some_and(|date| !date.is_null()),
        "case {:?}: a Map Run whose items are due reports a stopDate, got {:?}",
        "once the items are due",
        settled[0]
    );
    // A second read sees the same instant: a report does not move what it reports.
    let again = list_map_runs(&emulator, &execution).await;
    assert_eq!(
        again[0].get("stopDate"),
        settled[0].get("stopDate"),
        "two reads of one Map Run see one stopDate, got {again:?} after {settled:?}"
    );
}

/// Stopping the execution stops the Map Runs inside it: each one reports a `stopDate`
/// from then on, and that instant does not move afterwards.
#[tokio::test]
async fn stopping_the_execution_stops_its_map_runs() {
    let emulator = Emulator::new();
    // The `Wait` keeps the EXECUTION running long past the stop, so what the stop
    // does to the Map Run is what is observed.
    let definition = machine_definition(
        "M",
        json!({
            "M": distributed_map(None, Some("W")),
            "W": { "Type": "Wait", "Seconds": 120, "End": true },
        }),
    );
    let machine = create_machine(&emulator, "maprun-stopped", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", TWO_ITEMS).await;
    emulator
        .ok("StopExecution", &json!({ "executionArn": execution }))
        .await;

    let stopped = list_map_runs(&emulator, &execution).await;
    assert_eq!(
        stopped.len(),
        1,
        "the stopped execution still reports the Map Run it started, got {stopped:?}"
    );
    assert!(
        stopped[0].get("stopDate").is_some_and(|date| !date.is_null()),
        "a Map Run of a stopped execution reports a stopDate, got {:?}",
        stopped[0]
    );

    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    let later = list_map_runs(&emulator, &execution).await;
    assert_eq!(
        later[0].get("stopDate"),
        stopped[0].get("stopDate"),
        "the stop instant does not move once it is reported, got {later:?} after {stopped:?}"
    );
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

/// `maxResults` bounds the page and `nextToken` continues it, over every page size
/// from one to past the whole list: the pages together are the list, in order and
/// with no record reported twice.
#[tokio::test]
async fn list_map_runs_pages_by_max_results_and_next_token() {
    let emulator = Emulator::new();
    let definition = machine_definition(
        "A",
        json!({
            "A": distributed_map(None, Some("B")),
            "B": distributed_map(None, Some("C")),
            "C": distributed_map(None, None),
        }),
    );
    let machine = create_machine(&emulator, "maprun-paged", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", TWO_ITEMS).await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let whole: Vec<String> = list_map_runs(&emulator, &execution)
        .await
        .iter()
        .map(|map_run| string(map_run, "mapRunArn"))
        .collect();
    assert_eq!(
        whole.len(),
        3,
        "the execution entered three Distributed Map states, got {whole:?}"
    );

    // One row per page size: inside the list, exactly the list, and past it.
    for page in [1_usize, 2, 3, 4] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({ "executionArn": execution, "maxResults": page });
            if let Some(token) = &token {
                body["nextToken"] = Value::String(token.clone());
            }
            let listed = emulator.ok("ListMapRuns", &body).await;
            let items = listed
                .get("mapRuns")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                items.len() <= page,
                "case {page:?}: a page carries at most maxResults records, got {items:?}"
            );
            walked.extend(items.iter().map(|map_run| string(map_run, "mapRunArn")));
            token = listed
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                walked.len() <= whole.len(),
                "case {page:?}: paging must end, got {walked:?}"
            );
        }
        assert_eq!(
            walked, whole,
            "case {page:?}: the pages together are the whole list, got {walked:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// ListMapRuns -> DescribeMapRun: the Map Run read back by its own ARN
// ---------------------------------------------------------------------------

/// One row of the Map Run projection table: a Distributed Map state and what
/// `DescribeMapRun` must report for the Map Run it started.
struct DescribeCase {
    case: &'static str,
    state: Value,
    status: &'static str,
    max_concurrency: i64,
    tolerated_count: i64,
    tolerated_percentage: f64,
    total: i64,
    succeeded: i64,
    failed: i64,
}

/// The `maxConcurrency` a Map Run reports when its `Map` state set no limit.
///
/// RECORDED value, written out rather than taken from the implementation:
/// `DescribeMapRun` on a `Map` state that declares no `MaxConcurrency` answers
/// `2147483647` (`sfn.describe-map-run-default-fields-and-execution-counts`), which
/// is the largest value the modelled integer carries — the member's default of `0`
/// is what the Amazon States Language defines as no limit.
const UNLIMITED_CONCURRENCY: i64 = 2_147_483_647;

/// A processor that fails the items whose `ok` member is `false`.
fn ok_or_fail_processor() -> Value {
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

/// The Map Run record a Distributed Map left is read back by its own ARN, with the
/// configuration the state declared and the counts its items produced.
///
/// Rows vary along the configuration (`MaxConcurrency`, both tolerances) and along
/// how the items turned out (all succeeded, some failed inside the tolerance, all
/// failed past it).
#[tokio::test]
async fn describe_map_run_reports_the_record_the_distributed_map_left() {
    let emulator = Emulator::new();
    let rows = [
        DescribeCase {
            case: "every item succeeded, with no configuration of its own",
            state: distributed_map(None, None),
            status: "SUCCEEDED",
            max_concurrency: UNLIMITED_CONCURRENCY,
            tolerated_count: 0,
            tolerated_percentage: 0.0,
            total: 2,
            succeeded: 2,
            failed: 0,
        },
        DescribeCase {
            case: "a concurrency limit and a tolerated failure count",
            state: json!({
                "Type": "Map",
                "Items": [1, 2, 3],
                "MaxConcurrency": 1,
                "ToleratedFailureCount": 3,
                "ItemProcessor": {
                    "ProcessorConfig": { "Mode": "DISTRIBUTED" },
                    "StartAt": "P",
                    "States": { "P": { "Type": "Pass", "End": true } },
                },
                "End": true,
            }),
            status: "SUCCEEDED",
            max_concurrency: 1,
            tolerated_count: 3,
            tolerated_percentage: 0.0,
            total: 3,
            succeeded: 3,
            failed: 0,
        },
        DescribeCase {
            case: "one item of two failed, inside the tolerated percentage",
            state: json!({
                "Type": "Map",
                "Items": [{ "ok": true }, { "ok": false }],
                "ToleratedFailurePercentage": 50,
                "ItemProcessor": ok_or_fail_processor(),
                "End": true,
            }),
            status: "SUCCEEDED",
            max_concurrency: UNLIMITED_CONCURRENCY,
            tolerated_count: 0,
            tolerated_percentage: 50.0,
            total: 2,
            succeeded: 1,
            failed: 1,
        },
        DescribeCase {
            case: "every item failed, past what the state tolerates",
            state: json!({
                "Type": "Map",
                "Items": [{ "ok": false }, { "ok": false }],
                "ItemProcessor": ok_or_fail_processor(),
                "End": true,
            }),
            status: "FAILED",
            max_concurrency: UNLIMITED_CONCURRENCY,
            tolerated_count: 0,
            tolerated_percentage: 0.0,
            total: 2,
            succeeded: 0,
            failed: 2,
        },
    ];

    let mut started = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let definition = machine_definition("M", json!({ "M": row.state }));
        let machine = create_machine(&emulator, &format!("describe-{index}"), &definition).await;
        started.push(start_execution(&emulator, &machine, "only", TWO_ITEMS).await);
    }

    // Before the clock makes the items due, every Map Run is RUNNING with every item
    // still pending and no stopDate.
    for (row, execution) in rows.iter().zip(&started) {
        let case = row.case;
        let listed = list_map_runs(&emulator, execution).await;
        let arn = string(&listed[0], "mapRunArn");
        let described = emulator
            .ok("DescribeMapRun", &json!({ "mapRunArn": arn }))
            .await;
        assert_eq!(
            string(&described, "status"),
            "RUNNING",
            "case {case:?}: a Map Run whose items are still owed is RUNNING, got {described:?}"
        );
        assert_eq!(
            integer(&described, "itemCounts", "pending"),
            row.total,
            "case {case:?}: every item of a RUNNING Map Run is pending, got {described:?}"
        );
        assert!(
            described.get("stopDate").is_none_or(Value::is_null),
            "case {case:?}: a RUNNING Map Run reports no stopDate, got {described:?}"
        );
    }

    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for (row, execution) in rows.iter().zip(&started) {
        let case = row.case;
        let listed = list_map_runs(&emulator, execution).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: one Distributed Map state started one Map Run, got {listed:?}"
        );
        let arn = string(&listed[0], "mapRunArn");
        let described = emulator
            .ok("DescribeMapRun", &json!({ "mapRunArn": arn }))
            .await;

        assert_eq!(
            string(&described, "mapRunArn"),
            arn,
            "case {case:?}: the record answers for the ARN it was asked about, got {described:?}"
        );
        assert_eq!(
            string(&described, "executionArn"),
            *execution,
            "case {case:?}: the record names the execution that started it, got {described:?}"
        );
        assert_eq!(
            string(&described, "status"),
            row.status,
            "case {case:?}: expected status {}, got {described:?}",
            row.status
        );
        assert!(
            described.get("stopDate").is_some_and(|date| !date.is_null()),
            "case {case:?}: a finished Map Run reports its stopDate, got {described:?}"
        );
        assert_eq!(
            described.get("maxConcurrency").and_then(Value::as_i64),
            Some(row.max_concurrency),
            "case {case:?}: expected maxConcurrency {}, got {described:?}",
            row.max_concurrency
        );
        assert_eq!(
            described.get("toleratedFailureCount").and_then(Value::as_i64),
            Some(row.tolerated_count),
            "case {case:?}: expected toleratedFailureCount {}, got {described:?}",
            row.tolerated_count
        );
        assert_eq!(
            described
                .get("toleratedFailurePercentage")
                .and_then(Value::as_f64),
            Some(row.tolerated_percentage),
            "case {case:?}: expected toleratedFailurePercentage {}, got {described:?}",
            row.tolerated_percentage
        );
        assert_eq!(
            described.get("redriveCount").and_then(Value::as_i64),
            Some(0),
            "case {case:?}: a Map Run nobody redrove reports a redriveCount of 0, got {described:?}"
        );
        assert!(
            described.get("redriveDate").is_none_or(Value::is_null),
            "case {case:?}: a Map Run nobody redrove reports no redriveDate, got {described:?}"
        );

        // The items, and the child workflow executions that ran them, are counted as
        // the iterations turned out.
        for counts in ["itemCounts", "executionCounts"] {
            let actual = (
                integer(&described, counts, "total"),
                integer(&described, counts, "succeeded"),
                integer(&described, counts, "failed"),
                integer(&described, counts, "pending"),
                integer(&described, counts, "running"),
                integer(&described, counts, "aborted"),
                integer(&described, counts, "timedOut"),
                integer(&described, counts, "resultsWritten"),
            );
            let expected = (
                row.total,
                row.succeeded,
                row.failed,
                0,
                0,
                0,
                0,
                row.succeeded,
            );
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {counts} {expected:?}, got {described:?}"
            );
        }
    }
}

/// Every request `DescribeMapRun` cannot serve is refused with the modelled error for
/// what is wrong with it.
#[tokio::test]
async fn describe_map_run_refuses_what_it_cannot_serve() {
    let emulator = Emulator::new();
    let definition = machine_definition("M", json!({ "M": distributed_map(None, None) }));
    let machine = create_machine(&emulator, "describe-refusals", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", TWO_ITEMS).await;
    let listed = list_map_runs(&emulator, &execution).await;
    let arn = string(&listed[0], "mapRunArn");

    // (case, mapRunArn, expected status, expected error code)
    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "an ARN that is not an ARN at all",
            "not-an-arn".to_string(),
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
            format!("arn:aws:states:{REGION}:{ACCOUNT}:mapRun:describe-refusals/M:never"),
            404,
            "ResourceNotFound",
        ),
    ];

    for (case, map_run_arn, expected_status, expected) in &cases {
        let (status, parsed) = emulator
            .call("DescribeMapRun", &json!({ "mapRunArn": map_run_arn }))
            .await;
        let code = error_code(&parsed);
        assert_eq!(
            (status, code.as_str()),
            (*expected_status, *expected),
            "case {case:?}: expected {expected_status} {expected}, got {status} {code} with \
             {parsed:?}"
        );
    }

    // The refusals above changed nothing: the Map Run that does exist still answers.
    let described = emulator
        .ok("DescribeMapRun", &json!({ "mapRunArn": arn }))
        .await;
    assert_eq!(
        string(&described, "mapRunArn"),
        arn,
        "the stored Map Run is still served, got {described:?}"
    );
}

// ---------------------------------------------------------------------------
// Redrive
// ---------------------------------------------------------------------------

/// A redrive re-runs the SAME `Map` state, so the Map Run that state already has is
/// the one that runs again: it keeps its ARN, counts the redrive and reports the
/// instant of it, and no second Map Run appears for the one label.
#[tokio::test]
async fn redriving_the_execution_re_runs_the_map_run_it_already_started() {
    let emulator = Emulator::new();
    // Items that all fail leave the execution FAILED, which is what a redrive
    // continues from: "the execution status isn't SUCCEEDED".
    let definition = machine_definition(
        "M",
        json!({
            "M": {
                "Type": "Map",
                "Items": [{ "ok": false }, { "ok": false }],
                "ItemProcessor": ok_or_fail_processor(),
                "End": true,
            },
        }),
    );
    let machine = create_machine(&emulator, "maprun-redriven", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", TWO_ITEMS).await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let first = list_map_runs(&emulator, &execution).await;
    assert_eq!(
        first.len(),
        1,
        "the first attempt started one Map Run, got {first:?}"
    );
    let arn = string(&first[0], "mapRunArn");
    let described = emulator
        .ok("DescribeMapRun", &json!({ "mapRunArn": arn }))
        .await;
    assert_eq!(
        (
            string(&described, "status"),
            described.get("redriveCount").and_then(Value::as_i64)
        ),
        ("FAILED".to_string(), Some(0)),
        "case {:?}: expected a FAILED Map Run nobody redrove, got {described:?}",
        "before the redrive"
    );

    emulator
        .ok("RedriveExecution", &json!({ "executionArn": execution }))
        .await;

    let again = list_map_runs(&emulator, &execution).await;
    assert_eq!(
        again.len(),
        1,
        "the redrive re-ran the one Map state, so there is still one Map Run, got {again:?}"
    );
    assert_eq!(
        string(&again[0], "mapRunArn"),
        arn,
        "a re-run Map Run keeps the ARN a client is holding, got {again:?}"
    );
    assert!(
        again[0].get("stopDate").is_none_or(Value::is_null),
        "case {:?}: a Map Run running again reports no stopDate, got {:?}",
        "straight after the redrive",
        again[0]
    );
    let redriven = emulator
        .ok("DescribeMapRun", &json!({ "mapRunArn": arn }))
        .await;
    assert_eq!(
        string(&redriven, "status"),
        "RUNNING",
        "case {:?}: the re-run's items are owed to the clock again, got {redriven:?}",
        "straight after the redrive"
    );
    assert_eq!(
        redriven.get("redriveCount").and_then(Value::as_i64),
        Some(1),
        "one redrive is counted once, got {redriven:?}"
    );
    assert!(
        redriven.get("redriveDate").is_some_and(|date| !date.is_null()),
        "a redriven Map Run reports the date it was redriven, got {redriven:?}"
    );

    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    let settled = emulator
        .ok("DescribeMapRun", &json!({ "mapRunArn": arn }))
        .await;
    assert_eq!(
        (
            string(&settled, "status"),
            settled.get("redriveCount").and_then(Value::as_i64),
            integer(&settled, "itemCounts", "failed")
        ),
        ("FAILED".to_string(), Some(1), 2),
        "case {:?}: the re-run reached its own outcome, got {settled:?}",
        "once the re-run's items are due"
    );
    assert!(
        settled.get("stopDate").is_some_and(|date| !date.is_null()),
        "the finished re-run reports its stopDate, got {settled:?}"
    );
}

// ---------------------------------------------------------------------------
// What the OTHER readers of a Map Run ARN answer
// ---------------------------------------------------------------------------

/// `ListExecutions` on a Map Run ARN answers for the record the store holds: the
/// child workflow executions of a Map Run that EXISTS — none, because this emulator
/// runs a Distributed Map's items in the parent execution's own walk — and a 404 only
/// for an ARN no record answers for. A token this service never issued is still
/// refused on that branch.
#[tokio::test]
async fn list_executions_answers_for_the_map_run_the_store_holds() {
    let emulator = Emulator::new();
    let definition = machine_definition("M", json!({ "M": distributed_map(None, None) }));
    let machine = create_machine(&emulator, "maprun-children", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", TWO_ITEMS).await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    let arn = string(&list_map_runs(&emulator, &execution).await[0], "mapRunArn");

    let listed = emulator
        .ok("ListExecutions", &json!({ "mapRunArn": arn }))
        .await;
    assert_eq!(
        listed.get("executions").and_then(Value::as_array),
        Some(&Vec::new()),
        "a Map Run this emulator ran in the parent walk has no child execution \
         records, got {listed:?}"
    );
    assert!(
        listed.get("nextToken").is_none_or(Value::is_null),
        "an empty page carries no nextToken, got {listed:?}"
    );

    let (status, code) = emulator
        .err(
            "ListExecutions",
            &json!({ "mapRunArn": arn, "nextToken": "not-a-token" }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidToken"),
        "a token this service never issued is refused on the Map Run branch too, got \
         {status} {code}"
    );

    let (status, code) = emulator
        .err(
            "ListExecutions",
            &json!({ "mapRunArn": format!("{arn}-not-this-one") }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (404, "ResourceNotFound"),
        "a Map Run ARN no record answers for is a 404, got {status} {code}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// Every request `ListMapRuns` cannot serve is refused with the modelled error for
/// what is wrong with it, and never with an empty page.
#[tokio::test]
async fn list_map_runs_refuses_what_it_cannot_serve() {
    let emulator = Emulator::new();
    let definition = machine_definition("M", json!({ "M": distributed_map(None, None) }));
    let machine = create_machine(&emulator, "maprun-refusals", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", TWO_ITEMS).await;

    // (case, executionArn, nextToken, expected error code)
    let cases: Vec<(&str, String, Option<&str>, &str)> = vec![
        (
            "an ARN that is not an ARN at all",
            "not-an-arn".to_string(),
            None,
            "InvalidArn",
        ),
        (
            "another service's ARN",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:f"),
            None,
            "InvalidArn",
        ),
        (
            "a state machine ARN where an execution ARN is required",
            machine.clone(),
            None,
            "InvalidArn",
        ),
        (
            "a map run ARN where an execution ARN is required",
            format!("arn:aws:states:{REGION}:{ACCOUNT}:mapRun:maprun-refusals/M:abcd"),
            None,
            "InvalidArn",
        ),
        (
            "an execution ARN of another region",
            execution.replace(REGION, "eu-west-1"),
            None,
            "InvalidArn",
        ),
        (
            "an execution ARN with no execution name",
            format!("arn:aws:states:{REGION}:{ACCOUNT}:execution:maprun-refusals"),
            None,
            "InvalidArn",
        ),
        (
            "an execution this store never held",
            format!("arn:aws:states:{REGION}:{ACCOUNT}:execution:maprun-refusals:never"),
            None,
            "ExecutionDoesNotExist",
        ),
        (
            "an execution of a state machine that was never created",
            format!("arn:aws:states:{REGION}:{ACCOUNT}:execution:no-such-machine:only"),
            None,
            "ExecutionDoesNotExist",
        ),
        (
            "a page token this service never issued",
            execution.clone(),
            Some("not-a-token"),
            "InvalidToken",
        ),
    ];

    for (case, execution_arn, token, expected) in &cases {
        let mut body = json!({ "executionArn": execution_arn });
        if let Some(token) = token {
            body["nextToken"] = Value::String((*token).to_string());
        }
        let (status, parsed) = emulator.call("ListMapRuns", &body).await;
        let code = error_code(&parsed);
        assert_eq!(
            (status, code.as_str()),
            (400, *expected),
            "case {case:?}: expected 400 {expected}, got {status} {code} with {parsed:?}"
        );
    }

    // The refusals above changed nothing: the execution still reports its Map Run.
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    let listed = list_map_runs(&emulator, &execution).await;
    assert_eq!(
        listed.len(),
        1,
        "the execution's own Map Run is still reported, got {listed:?}"
    );
}
