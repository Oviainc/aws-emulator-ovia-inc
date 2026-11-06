// SPDX-License-Identifier: CC0-1.0

//! A `Map` state's `ItemBatcher`: how its items are GROUPED before they are iterated.
//!
//! "ItemBatcher — specifies how to batch the items ... each child workflow execution
//! then receives a batch of items as input": a `Map` state that declares one does not
//! hand one item to each iteration, it hands a BATCH — an object carrying the
//! `Items` array of that batch, and beside it the `BatchInput` the state built, which
//! every batch of the state carries the same copy of.
//!
//! How large a batch is, is what the member's two bounds say: `MaxItemsPerBatch` is how
//! many items one may hold, and `MaxInputBytesPerBatch` how many bytes — "the maximum
//! size of a batch" — so a batch is closed when the next item would pass either. A
//! state that declares neither bound puts every item in ONE batch, held to what a
//! state's document may carry at all ([`super::asl::MAX_PAYLOAD_BYTES`]).
//!
//! The `ItemSelector` is applied FIRST, to each item on its own, and the batches are
//! built out of what it produced — so a definition that declares both sees its
//! selector's projection inside `Items`.
//!
//! Nothing here reads a dataset or evaluates an expression: the numbers and the
//! `BatchInput` document arrive already resolved by the state's own query language, and
//! this module only groups. One item too large for the bound it was given is not
//! silently split or dropped: the state fails with the language's own data-limit error.

use serde_json::{Map, Value};

use super::integration::Failed;

/// The member a batch carries its items under: "Items — an array of items".
const ITEMS_MEMBER: &str = "Items";

/// The member a batch carries the state's own `BatchInput` under.
const BATCH_INPUT_MEMBER: &str = "BatchInput";

/// The members an `ItemBatcher` object carries, which is what one written with any
/// other is refused for.
pub const BATCHER_MEMBERS: &[&str] = &[
    "MaxItemsPerBatch",
    "MaxItemsPerBatchPath",
    "MaxInputBytesPerBatch",
    "MaxInputBytesPerBatchPath",
    "BatchInput",
];

/// The most BATCHES one `Map` state may be given: one per item at most, which is the
/// same ceiling the array a state iterates is held to
/// ([`super::asl::MAX_ARRAY_ITEMS`]).
pub const MAX_BATCHES: usize = super::asl::MAX_ARRAY_ITEMS;

/// The most bytes ALL of one state's batches may hold TOGETHER.
///
/// Each batch is bounded on its own, and a `BatchInput` is carried by EVERY batch — so
/// bounding one batch bounds no total: a state declaring one item per batch and a
/// `BatchInput` just under the per-batch bound makes as many copies of it as the dataset
/// holds items. The batches are a re-grouping of what a reader read, so together they
/// are held to the ceiling that read is
/// ([`super::item_reader::MAX_DATASET_BYTES`]), counted as they are built.
pub const MAX_BATCHED_BYTES: usize = super::item_reader::MAX_DATASET_BYTES;

/// How one `Map` state's items are grouped, as its `ItemBatcher` resolved.
#[derive(Debug, Clone, Default)]
pub struct Batching {
    /// `MaxItemsPerBatch`; `0` is the member absent, which sets no count bound.
    pub max_items: usize,
    /// `MaxInputBytesPerBatch`; `0` is the member absent, which leaves the bound a
    /// state's own document carries.
    pub max_bytes: usize,
    /// The `BatchInput` the state built, which every batch carries a copy of; `None`
    /// for a state that declares none.
    pub batch_input: Option<Value>,
}

/// `items` grouped into the batches `batching` describes — the documents the state's
/// iterations are handed, one per batch.
///
/// The `Err` is the `Map` state's own failure, under the language's data-limit error:
/// an item larger than one batch may carry cannot be placed in any batch, and batches
/// that together outgrow [`MAX_BATCHED_BYTES`] or outnumber [`MAX_BATCHES`] are not
/// built past those bounds — a `BatchInput` is copied into every batch, so what one
/// batch costs says nothing about what all of them cost.
pub fn batches(state: &str, items: Vec<Value>, batching: &Batching) -> Result<Vec<Value>, Failed> {
    let bytes_bound = match batching.max_bytes {
        0 => super::asl::MAX_PAYLOAD_BYTES,
        declared => declared.min(super::asl::MAX_PAYLOAD_BYTES),
    };
    // The `BatchInput` is part of every batch's own document, so what it costs is
    // charged against the bound before any item is placed.
    let overhead = batching
        .batch_input
        .as_ref()
        .map_or(0, |input| input.to_string().len());
    let mut batched: Vec<Value> = Vec::new();
    let mut held: Vec<Value> = Vec::new();
    let mut held_bytes = overhead;
    // What every batch BUILT SO FAR holds together, the `BatchInput` each carries
    // counted once per batch rather than once for the state.
    let mut emitted = 0usize;
    for item in items {
        let cost = item.to_string().len();
        if overhead.saturating_add(cost) > bytes_bound {
            return Err(too_large(format!(
                "an item of the 'Map' state '{state}' is {cost} bytes, more than the \
                 {bytes_bound} one batch of its 'ItemBatcher' may carry"
            )));
        }
        let full_by_count = batching.max_items > 0 && held.len() >= batching.max_items;
        let full_by_bytes = held_bytes.saturating_add(cost) > bytes_bound;
        if !held.is_empty() && (full_by_count || full_by_bytes) {
            emitted = emitted.saturating_add(held_bytes);
            exhausted(state, batched.len(), emitted)?;
            batched.push(one(&held, batching));
            held = Vec::new();
            held_bytes = overhead;
        }
        held_bytes = held_bytes.saturating_add(cost);
        held.push(item);
    }
    // The last batch is a batch like every other: a state's items do not have to
    // divide evenly into the bound it declared.
    if !held.is_empty() {
        emitted = emitted.saturating_add(held_bytes);
        exhausted(state, batched.len(), emitted)?;
        batched.push(one(&held, batching));
    }
    Ok(batched)
}

/// `Err` when the batches built so far have reached what ALL of one state's batches may
/// hold: `count` of them, holding `emitted` bytes together.
fn exhausted(state: &str, count: usize, emitted: usize) -> Result<(), Failed> {
    if count >= MAX_BATCHES {
        return Err(too_large(format!(
            "the 'ItemBatcher' of the 'Map' state '{state}' makes more than the {MAX_BATCHES} \
             batches this emulator runs"
        )));
    }
    if emitted > MAX_BATCHED_BYTES {
        return Err(too_large(format!(
            "the batches of the 'Map' state '{state}' hold at least {emitted} bytes together, \
             more than the {MAX_BATCHED_BYTES} this emulator carries"
        )));
    }
    Ok(())
}

/// A document that outgrew what a state may carry, under the language's own error name
/// for exactly that.
fn too_large(cause: String) -> Failed {
    Failed {
        error: super::asl::DATA_LIMIT_ERROR.to_string(),
        cause,
    }
}

/// One batch as the document an iteration is handed: its `Items`, and the state's
/// `BatchInput` beside them when it declares one.
fn one(items: &[Value], batching: &Batching) -> Value {
    let mut batch = Map::new();
    if let Some(input) = &batching.batch_input {
        batch.insert(BATCH_INPUT_MEMBER.to_string(), input.clone());
    }
    batch.insert(ITEMS_MEMBER.to_string(), Value::Array(items.to_vec()));
    Value::Object(batch)
}

/// How many items the batch `item` is holds, which is what a BATCHED iteration reports
/// as the count it processed.
///
/// Asked only of a state that declares an `ItemBatcher`: an unbatched iteration is
/// handed one item and processes one. A batch whose `Items` is not an array is a
/// document [`batches`] did not build, which is one item like any other.
#[must_use]
pub fn item_count(item: &Value) -> usize {
    item.get(ITEMS_MEMBER)
        .and_then(Value::as_array)
        .map_or(1, Vec::len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn items_are_grouped_into_the_batches_the_bound_describes() {
        let items: Vec<Value> = (1..=7).map(|index| json!({ "i": index })).collect();
        let rows: &[(&str, usize, Vec<usize>)] = &[
            ("three per batch leaves a short last batch", 3, vec![3, 3, 1]),
            ("one per batch is one item each", 1, vec![1, 1, 1, 1, 1, 1, 1]),
            ("a bound past the item count is one batch", 10, vec![7]),
            ("no count bound is one batch", 0, vec![7]),
        ];
        for (case, max_items, expected) in rows {
            let batching = Batching {
                max_items: *max_items,
                ..Batching::default()
            };
            let batched = batches("Fan", items.clone(), &batching).expect("the items batch");
            let sizes: Vec<usize> = batched.iter().map(item_count).collect();
            assert_eq!(
                &sizes, expected,
                "case {case:?}: expected batches of {expected:?}, got {sizes:?}"
            );
            let all: Vec<Value> = batched
                .iter()
                .flat_map(|batch| {
                    batch
                        .get("Items")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                })
                .collect();
            assert_eq!(
                all, items,
                "case {case:?}: every item is carried once, in order, got {all:?}"
            );
        }
    }

    #[test]
    fn a_batch_carries_the_states_batch_input_beside_its_items() {
        let batching = Batching {
            max_items: 2,
            batch_input: Some(json!({ "run": "r-1" })),
            ..Batching::default()
        };
        let batched = batches("Fan", vec![json!(1), json!(2), json!(3)], &batching)
            .expect("the items batch");
        assert_eq!(
            batched,
            vec![
                json!({ "BatchInput": { "run": "r-1" }, "Items": [1, 2] }),
                json!({ "BatchInput": { "run": "r-1" }, "Items": [3] }),
            ],
            "every batch carries the same BatchInput beside its own items, got {batched:?}"
        );
    }

    #[test]
    fn a_byte_bound_closes_a_batch_before_it_passes_the_bound() {
        // Each item's JSON text is `"aaaa"` — six bytes — so a bound of fourteen holds
        // two of them and not three.
        let items: Vec<Value> = (0..5).map(|_| json!("aaaa")).collect();
        let batching = Batching {
            max_bytes: 14,
            ..Batching::default()
        };
        let batched = batches("Fan", items, &batching).expect("the items batch");
        let sizes: Vec<usize> = batched.iter().map(item_count).collect();
        assert_eq!(
            sizes,
            vec![2, 2, 1],
            "a byte bound closes a batch before the next item passes it, got {sizes:?}"
        );
    }

    #[test]
    fn an_item_larger_than_one_batch_fails_the_state() {
        let refused = batches("Fan", vec![json!("aaaaaaaaaa")], &Batching { max_bytes: 4, ..Batching::default() })
            .expect_err("an item no batch can carry is refused");
        assert_eq!(
            refused.error,
            super::super::asl::DATA_LIMIT_ERROR,
            "an item too large for its batch is the language's data-limit failure, got {refused:?}"
        );
    }

    #[test]
    fn batches_that_together_outgrow_the_aggregate_bound_fail_the_state() {
        // A `BatchInput` is carried by EVERY batch, so one item per batch copies it once
        // per item: each batch is well inside its own bound and together they are not.
        let overhead = MAX_BATCHED_BYTES / 64;
        let input: String = "a".repeat(overhead);
        let items: Vec<Value> = (0..256).map(|index| json!(index)).collect();
        let refused = batches(
            "Fan",
            items,
            &Batching {
                max_items: 1,
                batch_input: Some(json!({ "p": input })),
                ..Batching::default()
            },
        )
        .expect_err("batches that together outgrow the bound are refused");
        assert_eq!(
            refused.error,
            super::super::asl::DATA_LIMIT_ERROR,
            "the aggregate bound is the language's data-limit failure, got {refused:?}"
        );
        assert!(
            refused.cause.contains("together"),
            "the cause names what all the batches hold together, got {:?}",
            refused.cause
        );
    }

    #[test]
    fn more_batches_than_this_emulator_runs_fail_the_state() {
        let items: Vec<Value> = (0..=MAX_BATCHES).map(|index| json!(index)).collect();
        let refused = batches(
            "Fan",
            items,
            &Batching {
                max_items: 1,
                ..Batching::default()
            },
        )
        .expect_err("more batches than this emulator runs is refused");
        assert_eq!(
            refused.error,
            super::super::asl::DATA_LIMIT_ERROR,
            "a batch count past the ceiling is the language's data-limit failure, got {refused:?}"
        );
    }

    #[test]
    fn a_document_no_batching_built_counts_as_one() {
        for case in [json!({ "id": 1 }), json!(7), json!([1, 2, 3])] {
            assert_eq!(
                item_count(&case),
                1,
                "case {case:?}: a document carrying no 'Items' array is one item, got {}",
                item_count(&case)
            );
        }
    }
}
