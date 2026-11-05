// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the arithmetic a reducing `UpdateConsumableResource` performs, over
//! the wire.
//!
//! The RULE under test, asserted over a GRID of stored totals crossed with reduction
//! quantities (every pair, not only the over-sized ones): a `REMOVE` reduces the stored
//! `totalQuantity` by `quantity` and saturates at zero — `max(0, total - quantity)` — so a
//! reduction larger than what the resource holds EMPTIES it with HTTP 200 instead of being
//! refused, and the total is never negative. The grid's rows differ along what the rule
//! generalises over: the stored total (zero, one, a small total, a large one, the largest
//! `Long`), the size of the reduction relative to it (nothing, part, exactly all, one past
//! all, far past all, the largest `Long`), the identifier form, and the resource type.
//!
//! The same saturation is asserted to PERSIST (a `Describe` reads the clamped total and the
//! `availableQuantity` derived from it), to be repeatable (draining an already-empty
//! resource stays at zero), and to leave the resource usable afterwards (an `ADD` fills it
//! again from zero).

use serde_json::{json, Value};

use crate::job_lifecycle::{arn, Batch};

/// The stored totals the grid starts a resource at: both published bounds of a non-negative
/// `Long` `totalQuantity` (source: the `CreateConsumableResourceRequest$totalQuantity` doc,
/// `smithy_query(query="doc", shape="CreateConsumableResourceRequest",
/// member="totalQuantity")`: "Must be non-negative"), and sizes in between.
const STARTING_TOTALS: [i64; 5] = [0, 1, 7, 1_000_000, i64::MAX];

/// How much each row asks to be removed, as an offset from the row's stored total: below
/// it, exactly it, and past it. `i64::MAX` is covered separately, as the absolute upper
/// bound of `quantity`.
const REDUCTION_OFFSETS: [i64; 6] = [-7, -1, 0, 1, 2, 1_000];

async fn create(batch: &Batch, body: Value) -> (u16, Value) {
    batch.call("/v1/createconsumableresource", body).await
}

async fn update(batch: &Batch, body: Value) -> (u16, Value) {
    batch.call("/v1/updateconsumableresource", body).await
}

async fn describe(batch: &Batch, reference: &str) -> (u16, Value) {
    batch
        .call(
            "/v1/describeconsumableresource",
            json!({"consumableResource": reference}),
        )
        .await
}

/// One row of the grid: a resource holding `total`, asked to give up `quantity`.
#[derive(Debug)]
struct Row {
    total: i64,
    quantity: i64,
    /// Whether the update names the resource by its ARN rather than by its name.
    by_arn: bool,
}

impl Row {
    /// The total the rule says the reduction leaves behind: the subtraction, floored at
    /// zero.
    fn expected(&self) -> i64 {
        self.total.saturating_sub(self.quantity).max(0)
    }
}

/// The grid: every starting total crossed with every reduction, plus the largest `Long` as
/// a reduction of each total. `by_arn` alternates so both documented identifier forms are
/// exercised across the grid.
fn grid() -> Vec<Row> {
    let mut rows = Vec::new();
    for total in STARTING_TOTALS {
        let mut quantities: Vec<i64> = REDUCTION_OFFSETS
            .iter()
            .map(|offset| total.saturating_add(*offset))
            // A reduction is non-negative, so an offset below zero clamps to zero rather
            // than becoming a refusable request.
            .map(|quantity| quantity.max(0))
            .collect();
        quantities.push(0);
        quantities.push(i64::MAX);
        quantities.sort_unstable();
        quantities.dedup();
        for (index, quantity) in quantities.into_iter().enumerate() {
            rows.push(Row {
                total,
                quantity,
                by_arn: index % 2 == 0,
            });
        }
    }
    rows
}

/// RULE: a `REMOVE` leaves `max(0, total - quantity)` — accepted, persisted, and never
/// negative — for every pair in the grid, under both documented `resourceType`s (the
/// arithmetic is the resource's total, not the type's).
#[tokio::test]
async fn a_remove_saturates_the_total_at_zero() {
    for resource_type in ["REPLENISHABLE", "NON_REPLENISHABLE"] {
        let batch = Batch::new();
        for (index, row) in grid().into_iter().enumerate() {
            let case = format!("{resource_type}/{row:?}");
            let name = format!("drain-{resource_type}-{index}");
            let expected_arn = arn(&format!("consumable-resource/{name}"));
            let (status, created) = create(
                &batch,
                json!({
                    "consumableResourceName": &name,
                    "resourceType": resource_type,
                    "totalQuantity": row.total,
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: the resource the row reduces is created, got {status} {created}"
            );

            let reference = if row.by_arn {
                expected_arn.clone()
            } else {
                name.clone()
            };
            let (status, updated) = update(
                &batch,
                json!({
                    "consumableResource": reference,
                    "operation": "REMOVE",
                    "quantity": row.quantity,
                }),
            )
            .await;
            let expected = row.expected();
            assert_eq!(
                status, 200,
                "case {case}: a REMOVE is accepted whatever its size, expected 200, got \
                 {status} {updated}"
            );
            assert_eq!(
                updated["totalQuantity"],
                json!(expected),
                "case {case}: expected the reduction to leave totalQuantity {expected}, got \
                 {updated}"
            );
            assert!(
                updated["totalQuantity"].as_i64().is_some_and(|total| total >= 0),
                "case {case}: a reduced totalQuantity is never negative, got {updated}"
            );
            assert_eq!(
                updated["consumableResourceName"],
                json!(name),
                "case {case}: expected the stored name {name:?}, got {updated}"
            );
            assert_eq!(
                updated["consumableResourceArn"],
                json!(expected_arn),
                "case {case}: expected the stored ARN {expected_arn:?}, got {updated}"
            );

            // Persisted, not merely answered.
            let (status, described) = describe(&batch, &name).await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200 from DescribeConsumableResource, got {status} \
                 {described}"
            );
            assert_eq!(
                described["totalQuantity"],
                json!(expected),
                "case {case}: expected the reader to report totalQuantity {expected}, got \
                 {described}"
            );
            assert_eq!(
                described["availableQuantity"],
                json!(expected),
                "case {case}: expected availableQuantity = totalQuantity - inUseQuantity = \
                 {expected}, got {described}"
            );
        }
    }
}

/// RULE applied twice: once a reduction has emptied a resource, every further reduction
/// leaves it at zero (the saturation is idempotent), and the resource still accepts an
/// `ADD` that fills it again — a drained resource is empty, not broken.
#[tokio::test]
async fn an_emptied_resource_stays_at_zero_and_fills_again() {
    let batch = Batch::new();
    // The rows differ in the size of the over-sized reduction that empties the resource.
    let rows: [(&str, i64, i64); 4] = [
        ("one past the total", 10, 11),
        ("twice the total", 10, 20),
        ("far past the total", 10, 1_000),
        ("the largest Long", 10, i64::MAX),
    ];

    for (case, total, quantity) in rows {
        let name = format!("emptied-{quantity}");
        let (status, created) = create(
            &batch,
            json!({"consumableResourceName": &name, "totalQuantity": total}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the resource is created, got {status} {created}"
        );

        // Three identical over-sized reductions: the first empties it, the rest keep it
        // empty. Each carries no `clientToken`, so each one is its own request.
        for attempt in 1..=3 {
            let (status, updated) = update(
                &batch,
                json!({
                    "consumableResource": &name,
                    "operation": "REMOVE",
                    "quantity": quantity,
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} (attempt {attempt}): expected 200, got {status} {updated}"
            );
            assert_eq!(
                updated["totalQuantity"],
                json!(0),
                "case {case:?} (attempt {attempt}): expected the emptied totalQuantity 0, \
                 got {updated}"
            );
        }

        // An ADD onto the emptied resource is ordinary arithmetic from zero.
        let (status, filled) = update(
            &batch,
            json!({"consumableResource": &name, "operation": "ADD", "quantity": 4}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 refilling the emptied resource, got {status} \
             {filled}"
        );
        assert_eq!(
            filled["totalQuantity"],
            json!(4),
            "case {case:?}: expected an ADD of 4 onto an emptied resource to leave 4, got \
             {filled}"
        );
        let (status, described) = describe(&batch, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from DescribeConsumableResource, got {status} \
             {described}"
        );
        assert_eq!(
            described["totalQuantity"],
            json!(4),
            "case {case:?}: expected the refilled total to persist as 4, got {described}"
        );
    }
}

/// RULE under `clientToken`: an over-sized reduction is a normal successful update, so an
/// identical repeat of it under one token answers what the first answered and applies
/// nothing further — the token's own rule, over the saturating path.
#[tokio::test]
async fn an_over_sized_reduction_is_idempotent_in_its_client_token() {
    let batch = Batch::new();
    let (status, created) = create(
        &batch,
        json!({"consumableResourceName": "tokened-drain", "totalQuantity": 25}),
    )
    .await;
    assert_eq!(status, 200, "the resource is created, got {status} {created}");

    let body = json!({
        "consumableResource": "tokened-drain",
        "clientToken": "drain-token",
        "operation": "REMOVE",
        "quantity": 1_000,
    });
    let mut first: Option<Value> = None;
    for attempt in 1..=3 {
        let (status, answered) = update(&batch, body.clone()).await;
        assert_eq!(
            status, 200,
            "case the drain's token (attempt {attempt}): expected 200, got {status} \
             {answered}"
        );
        assert_eq!(
            answered["totalQuantity"],
            json!(0),
            "case the drain's token (attempt {attempt}): expected the emptied \
             totalQuantity 0, got {answered}"
        );
        match &first {
            None => first = Some(answered),
            Some(expected) => assert_eq!(
                &answered, expected,
                "case the drain's token (attempt {attempt}): the same request must answer \
                 the same response, got {answered}"
            ),
        }
    }

    // A later ADD then proves the three repeats applied the drain exactly once: the total
    // starts from zero, not from a total some repeat reduced further.
    let (status, filled) = update(
        &batch,
        json!({"consumableResource": "tokened-drain", "operation": "ADD", "quantity": 6}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case refilling after the drain: expected 200, got {status} {filled}"
    );
    assert_eq!(
        filled["totalQuantity"],
        json!(6),
        "case refilling after the drain: expected 6, got {filled}"
    );
}
