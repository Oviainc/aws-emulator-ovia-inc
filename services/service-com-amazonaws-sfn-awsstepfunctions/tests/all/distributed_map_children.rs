// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CHILD WORKFLOW EXECUTIONS a Distributed Map state
//! starts, driven over this service's own wire protocol.
//!
//! The rules under test:
//!
//! 1. entering a `Map` state whose `ItemProcessor.ProcessorConfig.Mode` is
//!    `DISTRIBUTED` starts one child workflow execution per item of the state's item
//!    source — each with the `ItemSelector` projection of its item as its OWN input,
//!    what its iteration produced as its OWN output, the status that iteration
//!    reached, and an ARN of its own that the item's states read through
//!    `$$.Execution.Id` — while an INLINE `Map` starts none;
//! 2. a child is indexed under the Map Run that started it and nowhere else:
//!    `ListExecutions(mapRunArn)` enumerates exactly those children, paged by
//!    `maxResults`/`nextToken`, and `ListExecutions(stateMachineArn)` reports the
//!    executions started on the state machine and none of its Map Runs' children;
//! 3. `DescribeExecution` answers for a child from that same record, naming the Map
//!    Run in `mapRunArn`;
//! 4. a child is owed to the clock exactly as the Map Run around it is: it reports
//!    `RUNNING` until the instant its own item finished, and an execution stopped
//!    before then stops its children too.
//!
//! The table below spans the dimensions rule 1 generalises over: how many items the
//! state iterates (none, one, a few, more than its concurrency), where they come from
//! (`ItemsPath`, a literal `Items`), whether the state declares an `ItemSelector` (so
//! a child's input is a projection) or none (so it is the item itself), whether it
//! declares a `Label`, which `ExecutionType` its processor names, and how the
//! iterations turn out (all succeeded, some failed inside the state's tolerance).
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `string`) is the one
//! `state_machine_lifecycle` already builds; `tests/all.rs` is this binary's crate
//! root, so it is reached through `crate::state_machine_lifecycle`.

use serde_json::{json, Map, Value};

use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, string, Emulator,
    PAST_THE_EXECUTION_DELAY,
};

/// What one child workflow execution must report.
#[derive(Debug, Clone)]
struct ExpectedChild {
    /// The child's own `input`: the `ItemSelector` projection of its item, or the
    /// item itself where the state declares no selector.
    input: Value,
    /// The child's own `output`, for a child that succeeded; `None` for one that
    /// failed, which has none.
    output: Option<Value>,
    status: &'static str,
    /// The error name its iteration failed with, for a child that failed.
    error: Option<&'static str>,
}

/// One row of the table: a `Map` state, the execution input it runs over, and the
/// child executions it must start.
struct ChildrenCase {
    case: &'static str,
    state: Value,
    input: String,
    /// The `Label` of the Map state, which is its own name unless it declares one:
    /// what the Map Run and its children are reported under.
    label: &'static str,
    children: Vec<ExpectedChild>,
}

/// The `ItemProcessor` every row with an `ItemSelector` runs: one `Pass` that reports
/// what it computed from its item and the execution id the item's own states read.
///
/// `self` is `$$.Execution.Id`, so what each child PUT OUT names the execution it
/// ran as — which is what lets the ledger check below compare the ids the items
/// observed with the ARNs the listing returns.
fn doubling_processor(execution_type: &str) -> Value {
    json!({
        "ProcessorConfig": { "Mode": "DISTRIBUTED", "ExecutionType": execution_type },
        "StartAt": "Double",
        "States": {
            "Double": {
                "Type": "Pass",
                "Parameters": {
                    "id.$": "$.id",
                    "twice.$": "States.MathAdd($.v, $.v)",
                    "self.$": "$$.Execution.Id",
                },
                "End": true,
            },
        },
    })
}

/// The `ItemProcessor` a row with NO `ItemSelector` runs: one `Pass` that echoes the
/// whole input the child was handed, which is the item itself.
fn echo_processor() -> Value {
    json!({
        "ProcessorConfig": { "Mode": "DISTRIBUTED" },
        "StartAt": "Echo",
        "States": {
            "Echo": {
                "Type": "Pass",
                "Parameters": { "got.$": "$", "self.$": "$$.Execution.Id" },
                "End": true,
            },
        },
    })
}

/// The `ItemProcessor` the mixed row runs: an item carrying `"ok": true` succeeds and
/// one carrying `false` fails at a `Fail` state.
fn ok_or_fail_processor() -> Value {
    json!({
        "ProcessorConfig": { "Mode": "DISTRIBUTED" },
        "StartAt": "Check",
        "States": {
            "Check": {
                "Type": "Choice",
                "Choices": [{ "Variable": "$.ok", "BooleanEquals": true, "Next": "P" }],
                "Default": "F",
            },
            "P": {
                "Type": "Pass",
                "Parameters": { "id.$": "$.id", "self.$": "$$.Execution.Id" },
                "End": true,
            },
            "F": { "Type": "Fail", "Error": "ItemBroke", "Cause": "the item said so" },
        },
    })
}

/// The `ItemSelector` the rows that declare one use: the item's own members, plus a
/// member of the execution's input, so a child's input is a PROJECTION rather than
/// the item.
fn item_selector() -> Value {
    json!({
        "id.$": "$$.Map.Item.Value.id",
        "v.$": "$$.Map.Item.Value.v",
        "run.$": "$.run",
    })
}

/// `{"run": <run>, "items": [{"id": i, "v": i * 10}, ...]}` over `count` items.
fn numbered_items(run: &str, count: i64) -> String {
    let items: Vec<Value> = (0..count)
        .map(|index| json!({ "id": index, "v": index * 10 }))
        .collect();
    json!({ "run": run, "items": items }).to_string()
}

/// What the `doubling_processor` leaves for each of `numbered_items`' items.
fn doubled_children(run: &str, count: i64) -> Vec<ExpectedChild> {
    (0..count)
        .map(|index| ExpectedChild {
            input: json!({ "id": index, "v": index * 10, "run": run }),
            output: Some(json!({ "id": index, "twice": index * 20 })),
            status: "SUCCEEDED",
            error: None,
        })
        .collect()
}

/// One `Map` state, with the members a row adds to it.
fn map_state(items: Value, processor: Value, extra: Value) -> Value {
    let mut state = json!({ "Type": "Map", "ItemProcessor": processor, "End": true });
    let members = state.as_object_mut().expect("the state is an object");
    for (member, value) in items.as_object().expect("the item source is an object") {
        members.insert(member.clone(), value.clone());
    }
    for (member, value) in extra.as_object().expect("the extras are an object") {
        members.insert(member.clone(), value.clone());
    }
    state
}

fn cases() -> Vec<ChildrenCase> {
    vec![
        ChildrenCase {
            case: "eight items from ItemsPath, projected by an ItemSelector",
            state: map_state(
                json!({ "ItemsPath": "$.items" }),
                doubling_processor("STANDARD"),
                json!({ "ItemSelector": item_selector(), "MaxConcurrency": 8 }),
            ),
            input: numbered_items("eight", 8),
            label: "M",
            children: doubled_children("eight", 8),
        },
        ChildrenCase {
            case: "more items than the state runs at once",
            state: map_state(
                json!({ "ItemsPath": "$.items" }),
                doubling_processor("STANDARD"),
                json!({ "ItemSelector": item_selector(), "MaxConcurrency": 2 }),
            ),
            input: numbered_items("batched", 5),
            label: "M",
            children: doubled_children("batched", 5),
        },
        ChildrenCase {
            case: "one item, under a Label of the state's own",
            state: map_state(
                json!({ "ItemsPath": "$.items" }),
                doubling_processor("STANDARD"),
                json!({ "ItemSelector": item_selector(), "Label": "mapStateLabel" }),
            ),
            input: numbered_items("single", 1),
            label: "mapStateLabel",
            children: doubled_children("single", 1),
        },
        ChildrenCase {
            case: "a processor whose children are EXPRESS",
            state: map_state(
                json!({ "ItemsPath": "$.items" }),
                doubling_processor("EXPRESS"),
                json!({ "ItemSelector": item_selector() }),
            ),
            input: numbered_items("express", 3),
            label: "M",
            children: doubled_children("express", 3),
        },
        ChildrenCase {
            case: "no items at all",
            state: map_state(
                json!({ "ItemsPath": "$.items" }),
                doubling_processor("STANDARD"),
                json!({ "ItemSelector": item_selector() }),
            ),
            input: numbered_items("empty", 0),
            label: "M",
            children: Vec::new(),
        },
        ChildrenCase {
            case: "literal items and no ItemSelector, so a child's input is its item",
            state: map_state(
                json!({ "Items": [{ "n": 1 }, { "n": 2 }, { "n": 3 }] }),
                echo_processor(),
                json!({}),
            ),
            input: r#"{"run":"literal"}"#.to_string(),
            label: "M",
            children: (1..=3)
                .map(|n| ExpectedChild {
                    input: json!({ "n": n }),
                    output: Some(json!({ "got": { "n": n } })),
                    status: "SUCCEEDED",
                    error: None,
                })
                .collect(),
        },
        ChildrenCase {
            case: "items that failed inside what the state tolerates",
            state: map_state(
                json!({ "Items": [
                    { "id": 0, "ok": true },
                    { "id": 1, "ok": false },
                    { "id": 2, "ok": true },
                    { "id": 3, "ok": false },
                ] }),
                ok_or_fail_processor(),
                json!({ "ToleratedFailureCount": 2 }),
            ),
            input: r#"{"run":"mixed"}"#.to_string(),
            label: "M",
            children: (0..4)
                .map(|id| {
                    if id % 2 == 0 {
                        ExpectedChild {
                            input: json!({ "id": id, "ok": true }),
                            output: Some(json!({ "id": id })),
                            status: "SUCCEEDED",
                            error: None,
                        }
                    } else {
                        ExpectedChild {
                            input: json!({ "id": id, "ok": false }),
                            output: None,
                            status: "FAILED",
                            error: Some("ItemBroke"),
                        }
                    }
                })
                .collect(),
        },
    ]
}

/// The Map Runs of one execution, which must fit in one page.
async fn map_runs(emulator: &Emulator, execution_arn: &str) -> Vec<Value> {
    let listed = emulator
        .ok("ListMapRuns", &json!({ "executionArn": execution_arn }))
        .await;
    listed
        .get("mapRuns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("expected a mapRuns array in {listed:?}"))
}

/// Every execution `ListExecutions` reports for one selector, over pages of
/// `max_results` (the service's default when `None`).
async fn list_executions(emulator: &Emulator, selector: Value, max_results: Option<i64>) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = selector.clone();
        let members = request.as_object_mut().expect("the selector is an object");
        if let Some(max_results) = max_results {
            members.insert("maxResults".to_string(), json!(max_results));
        }
        if let Some(token) = &token {
            members.insert("nextToken".to_string(), json!(token));
        }
        let answered = emulator.ok("ListExecutions", &request).await;
        collected.extend(
            answered
                .get("executions")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("expected an executions array in {answered:?}"))
                .clone(),
        );
        token = answered
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            return collected;
        }
        assert!(
            collected.len() < 100,
            "the pages of one listing end, got {collected:?}"
        );
    }
}

/// `document` parsed, which must be the JSON text an execution member carries.
fn parsed(document: &str) -> Value {
    serde_json::from_str(document)
        .unwrap_or_else(|error| panic!("expected JSON in {document:?}, got {error}"))
}

/// `value` without its `self` member, and that member's value: the output every
/// processor above reports `$$.Execution.Id` through.
fn without_self(value: &Value) -> (Value, Option<String>) {
    let mut object: Map<String, Value> = value
        .as_object()
        .unwrap_or_else(|| panic!("expected an object output, got {value:?}"))
        .clone();
    let observed = object
        .remove("self")
        .and_then(|id| id.as_str().map(str::to_string));
    (Value::Object(object), observed)
}

// ---------------------------------------------------------------------------
// Rules 1, 2 and 3: one child per item, indexed under the Map Run alone
// ---------------------------------------------------------------------------

/// A Distributed Map starts one child workflow execution per item, each carrying its
/// own input, output and status; `ListExecutions` enumerates them for the Map Run and
/// excludes them from the state machine's own listing, and `DescribeExecution`
/// answers for each one from the same record.
#[tokio::test]
async fn a_distributed_map_starts_one_child_execution_per_item() {
    let emulator = Emulator::new();
    let rows = cases();

    // Every row is started first and read afterwards, so one wait covers the outcomes
    // the clock owed all of them.
    let mut started = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let name = format!("children-{index}");
        let definition =
            json!({ "StartAt": "M", "States": { "M": row.state } }).to_string();
        let machine = create_machine(&emulator, &name, &definition).await;
        let execution = start_execution(&emulator, &machine, "only", &row.input).await;
        started.push((machine, execution));
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for (row, (machine, execution)) in rows.iter().zip(started) {
        let case = row.case;
        let runs = map_runs(&emulator, &execution).await;
        assert_eq!(
            runs.len(),
            1,
            "case {case:?}: one Distributed Map state starts one Map Run, got {runs:?}"
        );
        let map_run_arn = string(&runs[0], "mapRunArn");
        let listed =
            list_executions(&emulator, json!({ "mapRunArn": map_run_arn }), None).await;
        assert_eq!(
            listed.len(),
            row.children.len(),
            "case {case:?}: expected one child execution per item ({}), got {listed:?}",
            row.children.len()
        );

        // Every child carries an ARN of its own, and that ARN is what the listing,
        // the describe and the item's own `$$.Execution.Id` all name.
        let mut arns: Vec<String> = listed
            .iter()
            .map(|child| string(child, "executionArn"))
            .collect();
        let total = arns.len();
        arns.sort();
        arns.dedup();
        assert_eq!(
            arns.len(),
            total,
            "case {case:?}: every child carries an ARN of its own, got {arns:?}"
        );

        let mut observed_ids: Vec<String> = Vec::new();
        for child in &listed {
            let arn = string(child, "executionArn");
            assert_eq!(
                (
                    child.get("mapRunArn").and_then(Value::as_str),
                    child.get("stateMachineArn").and_then(Value::as_str),
                    child.get("itemCount").and_then(Value::as_i64),
                ),
                (
                    Some(map_run_arn.as_str()),
                    Some(string(&runs[0], "stateMachineArn").as_str()),
                    Some(1),
                ),
                "case {case:?}: a listed child names its Map Run, the state machine \
                 the Map Run runs and the items it processed, got {child:?}"
            );
            // "The name of the execution": the last field of its own ARN.
            let name = string(child, "name");
            assert!(
                arn.ends_with(&format!(":{name}")),
                "case {case:?}: a child's ARN carries its name, got {arn} for {name}"
            );

            let described = describe_execution(&emulator, &arn).await;
            assert_eq!(
                described.get("mapRunArn").and_then(Value::as_str),
                Some(map_run_arn.as_str()),
                "case {case:?}: DescribeExecution names the Map Run that dispatched \
                 the child, got {described:?}"
            );
            // The other read of a child's record agrees with it: `mapRunArn` and
            // `label` are "returned only if the executionArn is a child workflow
            // execution that was started by a Distributed Map state".
            let for_execution = emulator
                .ok(
                    "DescribeStateMachineForExecution",
                    &json!({ "executionArn": arn }),
                )
                .await;
            assert_eq!(
                (
                    for_execution.get("mapRunArn").and_then(Value::as_str),
                    for_execution.get("label").and_then(Value::as_str),
                ),
                (Some(map_run_arn.as_str()), Some(row.label)),
                "case {case:?}: DescribeStateMachineForExecution names the Map Run \
                 and the Map state's label, got {for_execution:?}"
            );
            assert_eq!(
                described.get("status"),
                child.get("status"),
                "case {case:?}: the listing and the describe report one status, got \
                 {described:?} and {child:?}"
            );
            // The child's input decides which item it ran, so the expectation is
            // matched by it rather than by the order the listing happened to take.
            let input = parsed(&string(&described, "input"));
            let expected = row
                .children
                .iter()
                .find(|expected| expected.input == input)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: {input:?} is not the input of any item, \
                         expected one of {:?}",
                        row.children
                    )
                });
            let output = described
                .get("output")
                .and_then(Value::as_str)
                .map(parsed)
                .map(|output| {
                    let (rest, observed) = without_self(&output);
                    if let Some(observed) = observed {
                        observed_ids.push(observed);
                    }
                    rest
                });
            assert_eq!(
                (
                    described.get("status").and_then(Value::as_str),
                    output.as_ref(),
                    described.get("error").and_then(Value::as_str),
                ),
                (
                    Some(expected.status),
                    expected.output.as_ref(),
                    expected.error
                ),
                "case {case:?}: expected the child of {input:?} to report \
                 {expected:?}, got {described:?}"
            );
            // A child is redriven by redriving its Map Run, which is what it reports
            // rather than REDRIVABLE; one that SUCCEEDED is redrivable by nothing.
            let expected_redrive = if expected.status == "SUCCEEDED" {
                "NOT_REDRIVABLE"
            } else {
                "REDRIVABLE_BY_MAP_RUN"
            };
            assert_eq!(
                described.get("redriveStatus").and_then(Value::as_str),
                Some(expected_redrive),
                "case {case:?}: expected {expected_redrive} for the child of \
                 {input:?}, got {described:?}"
            );
        }

        // The ids the items observed through `$$.Execution.Id` are the ARNs the
        // listing returned: each item really ran as the execution it was listed as.
        // Only a child that SUCCEEDED put an output out to read one from.
        let mut succeeded_arns: Vec<String> = listed
            .iter()
            .filter(|child| child.get("status").and_then(Value::as_str) == Some("SUCCEEDED"))
            .map(|child| string(child, "executionArn"))
            .collect();
        succeeded_arns.sort();
        observed_ids.sort();
        assert_eq!(
            observed_ids, succeeded_arns,
            "case {case:?}: expected the ids the items read to be the listed ARNs, \
             got {observed_ids:?} against {succeeded_arns:?}"
        );

        // Rule 2: the children are indexed under the Map Run ALONE.
        let on_machine =
            list_executions(&emulator, json!({ "stateMachineArn": machine }), None).await;
        assert_eq!(
            on_machine
                .iter()
                .map(|item| string(item, "executionArn"))
                .collect::<Vec<String>>(),
            vec![execution.clone()],
            "case {case:?}: the state machine's listing holds the execution started \
             on it and none of its Map Run's children, got {on_machine:?}"
        );
        assert!(
            on_machine
                .iter()
                .all(|item| item.get("mapRunArn").is_none_or(Value::is_null)),
            "case {case:?}: mapRunArn is reported only for a request that named one, \
             got {on_machine:?}"
        );
    }
}

/// `ListExecutions(mapRunArn)` pages its children by `maxResults`/`nextToken`: every
/// page size from one to past the whole list reports the same children, in order and
/// with none reported twice.
#[tokio::test]
async fn list_executions_pages_a_map_runs_children() {
    let emulator = Emulator::new();
    let state = map_state(
        json!({ "ItemsPath": "$.items" }),
        doubling_processor("STANDARD"),
        json!({ "ItemSelector": item_selector() }),
    );
    let definition = json!({ "StartAt": "M", "States": { "M": state } }).to_string();
    let machine = create_machine(&emulator, "children-paged", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", &numbered_items("paged", 5)).await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    let map_run_arn = string(&map_runs(&emulator, &execution).await[0], "mapRunArn");

    let whole = list_executions(&emulator, json!({ "mapRunArn": map_run_arn }), None).await;
    assert_eq!(
        whole.len(),
        5,
        "the Map Run reports its five children in one page, got {whole:?}"
    );
    for page in [1, 2, 4, 5, 7] {
        let paged = list_executions(
            &emulator,
            json!({ "mapRunArn": map_run_arn }),
            Some(page),
        )
        .await;
        assert_eq!(
            paged, whole,
            "case {page:?}: the pages together are the whole listing, got {paged:?}"
        );
    }
}

/// An INLINE `Map` starts no child executions at all: its items are iterated inside
/// the execution that entered the state, so nothing is indexed under a Map Run it
/// never started.
#[tokio::test]
async fn an_inline_map_starts_no_child_executions() {
    let emulator = Emulator::new();
    let definition = json!({
        "StartAt": "M",
        "States": {
            "M": {
                "Type": "Map",
                "ItemsPath": "$.items",
                "ItemSelector": item_selector(),
                "ItemProcessor": {
                    "StartAt": "Double",
                    "States": {
                        "Double": {
                            "Type": "Pass",
                            "Parameters": { "id.$": "$.id", "self.$": "$$.Execution.Id" },
                            "End": true,
                        },
                    },
                },
                "End": true,
            },
        },
    })
    .to_string();
    let machine = create_machine(&emulator, "children-inline", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", &numbered_items("inline", 3)).await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    assert!(
        map_runs(&emulator, &execution).await.is_empty(),
        "an inline Map starts no Map Run"
    );
    let on_machine = list_executions(&emulator, json!({ "stateMachineArn": machine }), None).await;
    assert_eq!(
        on_machine
            .iter()
            .map(|item| string(item, "executionArn"))
            .collect::<Vec<String>>(),
        vec![execution.clone()],
        "an inline Map's items are not executions, got {on_machine:?}"
    );
    // The items DID run inside the parent: its output is what they produced, and each
    // one read the PARENT's execution id, because no child was started for it.
    let described = describe_execution(&emulator, &execution).await;
    let output = parsed(&string(&described, "output"));
    assert_eq!(
        output,
        json!([
            { "id": 0, "self": execution },
            { "id": 1, "self": execution },
            { "id": 2, "self": execution },
        ]),
        "an inline Map's iterations run as the execution that entered the state, got \
         {described:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: what the clock owes a child
// ---------------------------------------------------------------------------

/// A child workflow execution is `RUNNING` — so it has no `stopDate` — until the
/// instant its own item finished, and the ARN it is reported under does not change
/// when it settles.
#[tokio::test]
async fn a_child_execution_is_running_until_its_item_is_due() {
    let emulator = Emulator::new();
    let state = map_state(
        json!({ "ItemsPath": "$.items" }),
        doubling_processor("STANDARD"),
        json!({ "ItemSelector": item_selector() }),
    );
    let definition = json!({ "StartAt": "M", "States": { "M": state } }).to_string();
    let machine = create_machine(&emulator, "children-settle", &definition).await;
    let execution =
        start_execution(&emulator, &machine, "only", &numbered_items("settling", 2)).await;
    let map_run_arn = string(&map_runs(&emulator, &execution).await[0], "mapRunArn");

    let running = list_executions(&emulator, json!({ "mapRunArn": map_run_arn }), None).await;
    assert_eq!(
        running.len(),
        2,
        "the started execution reports its children straight away, got {running:?}"
    );
    for child in &running {
        assert_eq!(
            (
                child.get("status").and_then(Value::as_str),
                child.get("stopDate"),
            ),
            (Some("RUNNING"), None),
            "a child reports RUNNING until its item is due, got {child:?}"
        );
    }
    // A statusFilter selects by that same reported status.
    let filtered = list_executions(
        &emulator,
        json!({ "mapRunArn": map_run_arn, "statusFilter": "SUCCEEDED" }),
        None,
    )
    .await;
    assert!(
        filtered.is_empty(),
        "no child has succeeded yet, got {filtered:?}"
    );

    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    let settled = list_executions(&emulator, json!({ "mapRunArn": map_run_arn }), None).await;
    assert_eq!(
        settled
            .iter()
            .map(|child| string(child, "status"))
            .collect::<Vec<String>>(),
        vec!["SUCCEEDED".to_string(); 2],
        "every child reports the status its item reached once it is due, got {settled:?}"
    );
    assert_eq!(
        settled
            .iter()
            .map(|child| string(child, "executionArn"))
            .collect::<Vec<String>>(),
        running
            .iter()
            .map(|child| string(child, "executionArn"))
            .collect::<Vec<String>>(),
        "a child keeps the ARN it was reported under, got {settled:?} after {running:?}"
    );
    // A second read sees the same instant: a report does not move what it reports.
    let again = list_executions(&emulator, json!({ "mapRunArn": map_run_arn }), None).await;
    assert_eq!(
        again, settled,
        "two reads of one Map Run's children agree, got {again:?} after {settled:?}"
    );
    // The child's own history is the events IT produced: it started, and it reached
    // its terminal status.
    for child in &settled {
        let events = emulator
            .ok(
                "GetExecutionHistory",
                &json!({ "executionArn": string(child, "executionArn") }),
            )
            .await;
        let types: Vec<String> = events
            .get("events")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("expected an events array in {events:?}"))
            .iter()
            .map(|event| string(event, "type"))
            .collect();
        assert_eq!(
            types,
            vec![
                "ExecutionStarted".to_string(),
                "ExecutionSucceeded".to_string()
            ],
            "a child's history is the events it produced, got {events:?}"
        );
    }
}

/// Stopping the execution stops the child executions inside it: each one that had not
/// finished reports `ABORTED` from then on rather than settling to the outcome its
/// item had computed.
#[tokio::test]
async fn stopping_the_execution_stops_its_child_executions() {
    let emulator = Emulator::new();
    // The `Wait` keeps the EXECUTION running long past the stop, so what the stop
    // does to the children is what is observed.
    let state = map_state(
        json!({ "ItemsPath": "$.items" }),
        doubling_processor("STANDARD"),
        json!({ "ItemSelector": item_selector(), "Next": "W" }),
    );
    let mut state = state;
    state
        .as_object_mut()
        .expect("the state is an object")
        .remove("End");
    let definition = json!({
        "StartAt": "M",
        "States": {
            "M": state,
            "W": { "Type": "Wait", "Seconds": 120, "End": true },
        },
    })
    .to_string();
    let machine = create_machine(&emulator, "children-stopped", &definition).await;
    let execution =
        start_execution(&emulator, &machine, "only", &numbered_items("stopping", 2)).await;
    let map_run_arn = string(&map_runs(&emulator, &execution).await[0], "mapRunArn");
    emulator
        .ok("StopExecution", &json!({ "executionArn": execution }))
        .await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let stopped = list_executions(&emulator, json!({ "mapRunArn": map_run_arn }), None).await;
    assert_eq!(
        stopped
            .iter()
            .map(|child| string(child, "status"))
            .collect::<Vec<String>>(),
        vec!["ABORTED".to_string(); 2],
        "an execution stopped before its items were due stopped its children, got \
         {stopped:?}"
    );
    for child in &stopped {
        let described = describe_execution(&emulator, &string(child, "executionArn")).await;
        assert_eq!(
            (
                described.get("status").and_then(Value::as_str),
                described.get("output"),
            ),
            (Some("ABORTED"), None),
            "a stopped child reached no outcome of its own, got {described:?}"
        );
    }
}
