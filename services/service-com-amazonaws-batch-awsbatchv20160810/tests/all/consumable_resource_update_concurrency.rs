// SPDX-License-Identifier: CC0-1.0

//! `UpdateConsumableResource` under OVERLAPPING calls, over the wire.
//!
//! The sibling `consumable_resource_updates` suite drives one update at a time, which
//! cannot observe what a real client's thread pool does: several updates in flight
//! against one stored resource. Every case here runs its calls as spawned tasks on a
//! multi-thread runtime against one shared emulator, so the calls genuinely overlap, and
//! asserts the invariant the overlap must leave behind.
//!
//! RULE: the quantity change an update asks for is one serialized read-modify-write per
//! resource, so N overlapping updates compose exactly as the same N applied one after
//! another — every accepted update answers the total its OWN application left, and the
//! stored total afterwards is the arithmetic of all of them, with nothing lost and nothing
//! applied twice.
//!
//! The rows of each table differ along what the rule generalises over: the NUMBER of
//! overlapping callers, the quantity each one carries, which documented `operation` they
//! name (a mix of them included), how many resources are contended at once, and whether
//! the overlapping calls share a `clientToken`.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::job_lifecycle::{arn, Batch};

/// `UpdateConsumableResource`'s route (`smithy_query(query="operation",
/// operation="UpdateConsumableResource")`: POST /v1/updateconsumableresource).
const UPDATE: &str = "/v1/updateconsumableresource";

/// How many callers overlap, per row of the tables below: the smallest race there is, an
/// odd count, and two larger ones.
const RACER_COUNTS: [usize; 4] = [2, 3, 5, 10];

/// The quantity each overlapping update carries, per row: the lower bound of `quantity`
/// ("Must be a non-negative value"), one, and two larger values.
const QUANTITIES: [i64; 4] = [0, 1, 3, 1_000];

/// One racer's answer: its index, the HTTP status and the decoded body.
type Answer = (usize, u16, Value);

/// One emulator, shared by every racer, so the overlapping calls reach one store.
fn shared_batch() -> Arc<Batch> {
    Arc::new(Batch::new())
}

async fn create(batch: &Batch, body: Value) -> (u16, Value) {
    batch.call("/v1/createconsumableresource", body).await
}

/// The `totalQuantity` a `DescribeConsumableResource` of `reference` reports.
async fn stored_total(batch: &Batch, reference: &str) -> i64 {
    let (status, described) = batch
        .call(
            "/v1/describeconsumableresource",
            json!({"consumableResource": reference}),
        )
        .await;
    assert_eq!(
        status, 200,
        "reading back {reference:?}: expected 200 from DescribeConsumableResource, got \
         {status} {described}"
    );
    described["totalQuantity"]
        .as_i64()
        .unwrap_or_else(|| panic!("reading back {reference:?}: no totalQuantity in {described}"))
}

/// Create one resource at `total` and assert it was stored.
async fn resource_at(batch: &Batch, name: &str, total: i64) {
    let (status, created) = create(
        batch,
        json!({"consumableResourceName": name, "totalQuantity": total}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: the resource the race runs against is created at {total}, got \
         {status} {created}"
    );
}

/// Send every body in `bodies` to `UpdateConsumableResource` AT ONCE and answer one
/// [`Answer`] per racer, ordered by racer index.
///
/// Each call runs in its own spawned task holding a handle on the ONE shared emulator, so
/// on the multi-thread runtime these tests declare the calls really do overlap rather than
/// interleaving at a single task's await points.
async fn race(batch: &Arc<Batch>, bodies: Vec<Value>) -> Vec<Answer> {
    let mut racers = tokio::task::JoinSet::new();
    for (index, body) in bodies.into_iter().enumerate() {
        let batch = Arc::clone(batch);
        racers.spawn(async move {
            let (status, decoded) = batch.call(UPDATE, body).await;
            (index, status, decoded)
        });
    }
    let mut answers: Vec<Answer> = Vec::new();
    while let Some(joined) = racers.join_next().await {
        answers.push(joined.expect("a racer task must not panic"));
    }
    answers.sort_by_key(|(index, _, _)| *index);
    answers
}

/// Assert every racer was accepted, and answer the `totalQuantity` each one reported.
fn accepted_totals(case: &str, answers: &[Answer], name: &str, resource_arn: &str) -> Vec<i64> {
    answers
        .iter()
        .map(|(index, status, body)| {
            assert_eq!(
                *status, 200,
                "case {case}: racer {index} must be accepted with the modelled 200, got \
                 {status} {body}"
            );
            assert_eq!(
                body["consumableResourceName"],
                json!(name),
                "case {case}: racer {index} must answer the stored name {name:?}, got {body}"
            );
            assert_eq!(
                body["consumableResourceArn"],
                json!(resource_arn),
                "case {case}: racer {index} must answer the stored ARN {resource_arn:?}, got \
                 {body}"
            );
            body["totalQuantity"].as_i64().unwrap_or_else(|| {
                panic!("case {case}: racer {index} answered no totalQuantity: {body}")
            })
        })
        .collect()
}

/// The totals in `answered`, sorted — so a run's arrival order does not enter the
/// assertion, only the SET of totals the serialized applications must have produced.
fn sorted(answered: &[i64]) -> Vec<i64> {
    let mut totals = answered.to_vec();
    totals.sort_unstable();
    totals
}

/// RULE applied to `ADD`: N overlapping increases of `quantity` against a resource created
/// at zero each answer the total their own application left, so the answers are exactly
/// the first N multiples of `quantity` and the stored total settles at `N * quantity` —
/// no increase is lost to another racer's stale read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlapping_adds_lose_no_update() {
    for count in RACER_COUNTS {
        for quantity in QUANTITIES {
            let case = format!("{count} overlapping ADDs of {quantity}");
            let batch = shared_batch();
            let name = format!("add-{count}-{quantity}");
            let resource_arn = arn(&format!("consumable-resource/{name}"));
            resource_at(&batch, &name, 0).await;

            let bodies = (0..count)
                .map(|_| json!({"consumableResource": &name, "operation": "ADD", "quantity": quantity}))
                .collect();
            let answers = race(&batch, bodies).await;
            let totals = accepted_totals(&case, &answers, &name, &resource_arn);

            let expected: Vec<i64> = (1..=count as i64).map(|step| step * quantity).collect();
            assert_eq!(
                sorted(&totals),
                expected,
                "case {case}: each ADD answers the total its own application left, so the \
                 answers are {expected:?}, got {:?}",
                sorted(&totals)
            );
            let settled = stored_total(&batch, &name).await;
            assert_eq!(
                settled,
                count as i64 * quantity,
                "case {case}: the stored totalQuantity must settle at {}, got {settled}",
                count as i64 * quantity
            );
        }
    }
}

/// RULE applied to `REMOVE`: N overlapping reductions of `quantity` against a resource
/// holding exactly `N * quantity` each answer their own application's total, so the
/// answers are every multiple below the start and the resource settles at zero.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlapping_removes_lose_no_update() {
    for count in RACER_COUNTS {
        for quantity in QUANTITIES {
            let case = format!("{count} overlapping REMOVEs of {quantity}");
            let batch = shared_batch();
            let name = format!("remove-{count}-{quantity}");
            let resource_arn = arn(&format!("consumable-resource/{name}"));
            let start = count as i64 * quantity;
            resource_at(&batch, &name, start).await;

            let bodies = (0..count)
                .map(|_| {
                    json!({
                        "consumableResource": &name,
                        "operation": "REMOVE",
                        "quantity": quantity,
                    })
                })
                .collect();
            let answers = race(&batch, bodies).await;
            let totals = accepted_totals(&case, &answers, &name, &resource_arn);

            let expected: Vec<i64> = (0..count as i64).map(|step| step * quantity).collect();
            assert_eq!(
                sorted(&totals),
                expected,
                "case {case}: each REMOVE answers the total its own application left, so the \
                 answers are {expected:?}, got {:?}",
                sorted(&totals)
            );
            let settled = stored_total(&batch, &name).await;
            assert_eq!(
                settled, 0,
                "case {case}: {count} reductions of {quantity} from {start} must settle at \
                 0, got {settled}"
            );
        }
    }
}

/// RULE applied to a MIX of operations: `count` increases and `count` reductions of the
/// same quantity overlapping on one resource that starts holding `count * quantity` (so no
/// interleaving can take it below zero) are all accepted, every intermediate total stays
/// inside the window the arithmetic allows, and the resource settles back at where it
/// started.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlapping_adds_and_removes_settle_at_their_sum() {
    for count in RACER_COUNTS {
        for quantity in [1, 3, 1_000] {
            let case = format!("{count} ADDs and {count} REMOVEs of {quantity}");
            let batch = shared_batch();
            let name = format!("mixed-{count}-{quantity}");
            let resource_arn = arn(&format!("consumable-resource/{name}"));
            let start = count as i64 * quantity;
            resource_at(&batch, &name, start).await;

            // Interleaved, so neither operation is all-first in arrival order.
            let bodies = (0..2 * count)
                .map(|index| {
                    let operation = if index % 2 == 0 { "ADD" } else { "REMOVE" };
                    json!({
                        "consumableResource": &name,
                        "operation": operation,
                        "quantity": quantity,
                    })
                })
                .collect();
            let answers = race(&batch, bodies).await;
            let totals = accepted_totals(&case, &answers, &name, &resource_arn);

            let ceiling = 2 * start;
            for (racer, total) in totals.iter().enumerate() {
                assert!(
                    (0..=ceiling).contains(total),
                    "case {case}: racer {racer}'s total must stay within 0..={ceiling} — the \
                     window serialized applications allow — got {total}"
                );
            }
            let settled = stored_total(&batch, &name).await;
            assert_eq!(
                settled, start,
                "case {case}: as many increases as reductions must settle back at {start}, \
                 got {settled}"
            );
        }
    }
}

/// RULE applied to `SET`: overlapping updates that each make the quantity a total of their
/// own are all accepted, each answers the total it asked for, and the resource settles at
/// one writer's value whole — never a value no racer asked for and never a merge of two.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlapping_sets_settle_at_one_writers_total() {
    for count in RACER_COUNTS {
        let case = format!("{count} overlapping SETs of distinct totals");
        let batch = shared_batch();
        let name = format!("set-{count}");
        let resource_arn = arn(&format!("consumable-resource/{name}"));
        resource_at(&batch, &name, 7).await;

        let requested: Vec<i64> = (0..count as i64).map(|index| 100 + index).collect();
        let bodies = requested
            .iter()
            .map(|total| json!({"consumableResource": &name, "operation": "SET", "quantity": total}))
            .collect();
        let answers = race(&batch, bodies).await;
        let totals = accepted_totals(&case, &answers, &name, &resource_arn);

        for (racer, (answered, asked)) in totals.iter().zip(requested.iter()).enumerate() {
            assert_eq!(
                answered, asked,
                "case {case}: racer {racer} set the total to {asked}, so that is the total it \
                 must answer, got {answered}"
            );
        }
        let settled = stored_total(&batch, &name).await;
        assert!(
            requested.contains(&settled),
            "case {case}: the stored total must be one of the values a racer set \
             ({requested:?}), got {settled}"
        );
    }
}

/// RULE applied per RESOURCE: overlapping updates to DIFFERENT resources are each applied
/// to the resource they name, so every one of them settles at its own arithmetic and no
/// quantity crosses over.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlapping_updates_to_different_resources_keep_their_own_totals() {
    // (how many resources are contended at once, how many updates each one gets)
    for (resources, per_resource) in [(2usize, 2usize), (3, 5), (5, 3)] {
        let case = format!("{resources} resources, {per_resource} overlapping ADDs each");
        let batch = shared_batch();
        let names: Vec<String> = (0..resources)
            .map(|index| format!("cross-{resources}-{per_resource}-{index}"))
            .collect();
        for (index, name) in names.iter().enumerate() {
            resource_at(&batch, name, index as i64).await;
        }

        // Each resource's quantity differs, so a quantity applied to the wrong record
        // shows up as a wrong total rather than cancelling out.
        let quantity = |index: usize| index as i64 + 1;
        let bodies = (0..resources * per_resource)
            .map(|slot| {
                let index = slot % resources;
                json!({
                    "consumableResource": &names[index],
                    "operation": "ADD",
                    "quantity": quantity(index),
                })
            })
            .collect();
        let answers = race(&batch, bodies).await;
        for (racer, status, body) in &answers {
            assert_eq!(
                *status, 200,
                "case {case}: racer {racer} must be accepted with the modelled 200, got \
                 {status} {body}"
            );
        }

        for (index, name) in names.iter().enumerate() {
            let expected = index as i64 + per_resource as i64 * quantity(index);
            let settled = stored_total(&batch, name).await;
            assert_eq!(
                settled, expected,
                "case {case}: {name} started at {index} and took {per_resource} increases of \
                 {}, so it must settle at {expected}, got {settled}",
                quantity(index)
            );
        }
    }
}

/// RULE applied to `clientToken`: identical requests under ONE token are THE SAME request
/// even when they overlap — every one of them is accepted with the same response, and the
/// arithmetic is applied exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_client_token_applies_once_even_when_its_repeats_overlap() {
    for count in RACER_COUNTS {
        for (operation, start, applied) in [("ADD", 10, 13), ("REMOVE", 10, 7), ("SET", 10, 3)] {
            let case = format!("{count} overlapping {operation}s under one clientToken");
            let batch = shared_batch();
            let name = format!("token-{operation}-{count}");
            let resource_arn = arn(&format!("consumable-resource/{name}"));
            resource_at(&batch, &name, start).await;

            let bodies = (0..count)
                .map(|_| {
                    json!({
                        "consumableResource": &name,
                        "operation": operation,
                        "quantity": 3,
                        "clientToken": format!("token-{operation}-{count}"),
                    })
                })
                .collect();
            let answers = race(&batch, bodies).await;
            let totals = accepted_totals(&case, &answers, &name, &resource_arn);

            for (racer, total) in totals.iter().enumerate() {
                assert_eq!(
                    *total, applied,
                    "case {case}: every repeat answers the one application's total \
                     {applied}, got {total} from racer {racer}"
                );
            }
            let settled = stored_total(&batch, &name).await;
            assert_eq!(
                settled, applied,
                "case {case}: {count} identical requests under one token apply once, \
                 leaving {applied}, got {settled}"
            );
        }
    }
}
