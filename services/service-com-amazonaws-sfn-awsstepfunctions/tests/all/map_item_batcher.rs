// SPDX-License-Identifier: CC0-1.0

//! A `Map` state's `ItemBatcher`, driven over this service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`) through
//! the production `MultiServiceRouter`.
//!
//! The rule under test: a `Map` state that declares an `ItemBatcher` iterates GROUPS of
//! its array rather than single items, and each iteration is handed one group as the
//! `Items` member of a document — beside the fixed `BatchInput` payload the field may
//! declare. How large a group is, is what the field's own bounds state
//! (`MaxItemsPerBatch` and `MaxInputBytesPerBatch`, each also in its `*Path` form), and
//! a field that declares neither puts every item in one group. The `ItemSelector`
//! projects the INDIVIDUAL items, so it applies before the grouping and what the
//! iterations receive are the batches of what it produced.
//!
//! The rows differ along what the rule generalises over: which bound the field declares
//! and in which form, whether the items divide evenly by it, whether a fixed input is
//! carried, whether a per-item projection is declared, and whether the processor runs
//! inline or distributed.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`,
//! `describe_execution`) is the one `state_machine_lifecycle` builds; `tests/all.rs` is
//! this binary's crate root, so it is reached through `crate::<suite>`.

use serde_json::{json, Value};

use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, Emulator, PAST_THE_EXECUTION_DELAY,
};

/// The array every row iterates, as the execution's input.
const INPUT: &str = r#"{"items":[1,2,3,4,5],"perBatch":2}"#;

/// A definition whose one `Map` state iterates `$.items` with `batcher` as its
/// `ItemBatcher` and `selector` as its `ItemSelector`, and whose processor hands each
/// iteration's own input straight back — so the state's result IS what the iterations
/// were handed.
fn definition(batcher: Value, selector: Option<Value>, mode: &str) -> String {
    let mut state = json!({
        "Type": "Map",
        "ItemsPath": "$.items",
        "ItemBatcher": batcher,
        "ItemProcessor": {
            "ProcessorConfig": { "Mode": mode },
            "StartAt": "Echo",
            "States": { "Echo": { "Type": "Pass", "End": true } }
        },
        "End": true,
    });
    if let Some(selector) = selector {
        state
            .as_object_mut()
            .expect("the state is an object")
            .insert("ItemSelector".to_string(), selector);
    }
    json!({ "StartAt": "Batch", "States": { "Batch": state } }).to_string()
}

/// One row: what the `ItemBatcher` declares, what the state's `ItemSelector` declares,
/// the processor's mode, and the document each iteration must be handed.
struct Row {
    case: &'static str,
    batcher: Value,
    selector: Option<Value>,
    mode: &'static str,
    batches: Value,
}

/// Every row's execution is started, the clock is let run once, and each row's output —
/// the array of the documents its iterations were handed — is asserted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_map_state_hands_each_iteration_the_batch_its_item_batcher_groups() {
    let rows = [
        Row {
            case: "a count that leaves a short last batch",
            batcher: json!({ "MaxItemsPerBatch": 2 }),
            selector: None,
            mode: "INLINE",
            batches: json!([
                { "Items": [1, 2] },
                { "Items": [3, 4] },
                { "Items": [5] },
            ]),
        },
        Row {
            case: "a count larger than the array, which is one batch",
            batcher: json!({ "MaxItemsPerBatch": 10 }),
            selector: None,
            mode: "INLINE",
            batches: json!([{ "Items": [1, 2, 3, 4, 5] }]),
        },
        Row {
            case: "no bound at all, which is one batch of everything",
            batcher: json!({}),
            selector: None,
            mode: "INLINE",
            batches: json!([{ "Items": [1, 2, 3, 4, 5] }]),
        },
        Row {
            case: "the count taken from the input through its path form",
            batcher: json!({ "MaxItemsPerBatchPath": "$.perBatch" }),
            selector: None,
            mode: "INLINE",
            batches: json!([
                { "Items": [1, 2] },
                { "Items": [3, 4] },
                { "Items": [5] },
            ]),
        },
        Row {
            case: "a byte bound, which groups by the size of the items",
            // "MaxInputBytesPerBatch — the maximum size of a batch" is counted over the
            // items' own JSON, and each item here is one digit: two bytes hold two of
            // them and a third would pass the bound.
            batcher: json!({ "MaxInputBytesPerBatch": 2 }),
            selector: None,
            mode: "INLINE",
            batches: json!([
                { "Items": [1, 2] },
                { "Items": [3, 4] },
                { "Items": [5] },
            ]),
        },
        Row {
            case: "a fixed input, which every batch carries beside its items",
            batcher: json!({ "MaxItemsPerBatch": 3, "BatchInput": { "factCheck": "true" } }),
            selector: None,
            mode: "INLINE",
            batches: json!([
                { "BatchInput": { "factCheck": "true" }, "Items": [1, 2, 3] },
                { "BatchInput": { "factCheck": "true" }, "Items": [4, 5] },
            ]),
        },
        Row {
            case: "a per-item projection, applied to each item before the grouping",
            batcher: json!({ "MaxItemsPerBatch": 2 }),
            selector: Some(json!({ "n.$": "$$.Map.Item.Value", "at.$": "$$.Map.Item.Index" })),
            mode: "INLINE",
            batches: json!([
                { "Items": [{ "n": 1, "at": 0 }, { "n": 2, "at": 1 }] },
                { "Items": [{ "n": 3, "at": 2 }, { "n": 4, "at": 3 }] },
                { "Items": [{ "n": 5, "at": 4 }] },
            ]),
        },
        Row {
            case: "a distributed processor, whose child executions run one batch each",
            batcher: json!({ "MaxItemsPerBatch": 4 }),
            selector: None,
            mode: "DISTRIBUTED",
            batches: json!([
                { "Items": [1, 2, 3, 4] },
                { "Items": [5] },
            ]),
        },
    ];

    let emulator = Emulator::new();
    let mut started = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let machine = create_machine(
            &emulator,
            &format!("batched-{index}"),
            &definition(row.batcher.clone(), row.selector.clone(), row.mode),
        )
        .await;
        started.push(start_execution(&emulator, &machine, "only", INPUT).await);
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for (row, execution) in rows.iter().zip(started) {
        let case = row.case;
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected SUCCEEDED, got {described:?}"
        );
        let output: Value = described
            .get("output")
            .and_then(Value::as_str)
            .map(|text| serde_json::from_str(text).expect("the output is JSON"))
            .unwrap_or_else(|| panic!("case {case:?}: expected an output in {described:?}"));
        assert_eq!(
            output, row.batches,
            "case {case:?}: expected the iterations to be handed {}, got {output}",
            row.batches
        );
    }
}

/// An `ItemBatcher` that is not an object at all is a definition the walk cannot run:
/// the execution fails with the ASL runtime error naming the state, and nothing is
/// iterated.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_item_batcher_that_is_not_an_object_is_not_run() {
    let emulator = Emulator::new();
    let machine = create_machine(
        &emulator,
        "batcher-not-an-object",
        &definition(json!("every other item"), None, "INLINE"),
    )
    .await;
    let execution = start_execution(&emulator, &machine, "only", INPUT).await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let described = describe_execution(&emulator, &execution).await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("FAILED"),
        "expected FAILED, got {described:?}"
    );
    assert_eq!(
        described.get("error").and_then(Value::as_str),
        Some("States.Runtime"),
        "expected the ASL runtime error, got {described:?}"
    );
    let cause = described
        .get("cause")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        cause.contains("'ItemBatcher'") && cause.contains("every other item"),
        "expected the cause to name the member and what it was written as, got {cause:?}"
    );
}
