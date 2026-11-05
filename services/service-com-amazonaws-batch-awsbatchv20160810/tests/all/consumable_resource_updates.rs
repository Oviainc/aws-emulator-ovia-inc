// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateConsumableResource`, over the wire.
//!
//! Every case drives a real restJson1 request through the router the emulator mounts, so
//! what is asserted is the response a `batch` client gets.
//!
//! The rules under test, each over a SET of inputs:
//!
//! * an update applies the request's `quantity` to the stored `totalQuantity` the way
//!   `operation` says — `SET` makes it the new total, `ADD` increases it, `REMOVE` reduces
//!   it, saturating at zero — persists the result and answers the resource's name, its ARN
//!   and that total, which `DescribeConsumableResource` then reads back;
//! * the identifier is resolved by name or by ARN, the two forms `consumableResource`
//!   documents, and an identifier that resolves to nothing is refused rather than created;
//! * a `REMOVE` whose `quantity` is larger than the stored total empties the resource
//!   instead of being refused: the total saturates at zero and never goes negative;
//! * a request the service cannot apply is refused with the modelled `ClientException` and
//!   leaves the stored quantity exactly as it was: an `operation` outside the documented
//!   three, a negative `quantity`, and an `ADD` past the largest `Long`;
//! * `clientToken` makes an identical repeat THE SAME request: both calls succeed with the
//!   same response and the update happens once, while an update carrying no token is a new
//!   request every time.

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state::CONSUMABLE_RESOURCE_UPDATE_OPERATIONS;

use crate::job_lifecycle::{arn, error_code, error_message, Batch};

/// The `operation` values an update may name, and what each one does to the stored total.
/// Source: the `UpdateConsumableResourceRequest$operation` doc
/// (`smithy_query(query="doc", shape="UpdateConsumableResourceRequest",
/// member="operation")`): "Must be one of: SET ... ADD ... REMOVE".
const DOCUMENTED_OPERATIONS: [&str; 3] = ["SET", "ADD", "REMOVE"];

/// The `totalQuantity` every resource in the arithmetic table starts at.
const STARTING_TOTAL: i64 = 100;

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

/// The `totalQuantity` a describe of `reference` reports.
async fn stored_total(batch: &Batch, reference: &str) -> i64 {
    let (status, described) = describe(batch, reference).await;
    assert_eq!(
        status, 200,
        "reading back {reference:?}: expected 200, got {status} {described}"
    );
    described["totalQuantity"]
        .as_i64()
        .unwrap_or_else(|| panic!("reading back {reference:?}: no totalQuantity in {described}"))
}

/// One update applied to the resource the table walks, and the total it must leave behind.
struct Step {
    /// What this row varies, named in every assertion.
    label: &'static str,
    /// The request members beyond `consumableResource`.
    body: Value,
    /// Whether the request names the resource by its ARN rather than by its name.
    by_arn: bool,
    expected_total: i64,
}

/// The rows span what the rule generalises over: each documented `operation`, whether
/// `operation` and `quantity` are given at all, the published bounds of `quantity` (a
/// `Long`), a reduction to exactly zero, and both identifier forms.
fn steps() -> Vec<Step> {
    vec![
        Step {
            label: "ADD increases the total",
            body: json!({"operation": "ADD", "quantity": 50}),
            by_arn: false,
            expected_total: 150,
        },
        Step {
            label: "REMOVE reduces the total",
            body: json!({"operation": "REMOVE", "quantity": 30}),
            by_arn: false,
            expected_total: 120,
        },
        Step {
            label: "ADD named by ARN, not by name",
            body: json!({"operation": "ADD", "quantity": 5}),
            by_arn: true,
            expected_total: 125,
        },
        Step {
            label: "SET makes the quantity the new total",
            body: json!({"operation": "SET", "quantity": 7}),
            by_arn: false,
            expected_total: 7,
        },
        Step {
            label: "ADD at the lower bound of quantity changes nothing",
            body: json!({"operation": "ADD", "quantity": 0}),
            by_arn: false,
            expected_total: 7,
        },
        Step {
            label: "REMOVE of the whole total leaves zero",
            body: json!({"operation": "REMOVE", "quantity": 7}),
            by_arn: true,
            expected_total: 0,
        },
        Step {
            label: "REMOVE at the lower bound of quantity, against a zero total",
            body: json!({"operation": "REMOVE", "quantity": 0}),
            by_arn: false,
            expected_total: 0,
        },
        Step {
            label: "no operation: the quantity is the new total",
            body: json!({"quantity": 42}),
            by_arn: false,
            expected_total: 42,
        },
        Step {
            label: "a quantity the request does not name applies nothing",
            body: json!({"operation": "SET"}),
            by_arn: false,
            expected_total: 42,
        },
        Step {
            label: "neither operation nor quantity applies nothing",
            body: json!({}),
            by_arn: true,
            expected_total: 42,
        },
        Step {
            label: "ADD up to the largest Long the model admits",
            body: json!({"operation": "ADD", "quantity": i64::MAX - 42}),
            by_arn: false,
            expected_total: i64::MAX,
        },
        Step {
            label: "REMOVE the largest Long, back to zero",
            body: json!({"operation": "REMOVE", "quantity": i64::MAX}),
            by_arn: false,
            expected_total: 0,
        },
        Step {
            label: "SET to the largest Long",
            body: json!({"operation": "SET", "quantity": i64::MAX}),
            by_arn: true,
            expected_total: i64::MAX,
        },
        Step {
            label: "SET a small total to over-remove from",
            body: json!({"operation": "SET", "quantity": 5}),
            by_arn: false,
            expected_total: 5,
        },
        Step {
            label: "REMOVE one past the total saturates at zero",
            body: json!({"operation": "REMOVE", "quantity": 6}),
            by_arn: false,
            expected_total: 0,
        },
        Step {
            label: "REMOVE against a zero total stays at zero",
            body: json!({"operation": "REMOVE", "quantity": 1_000}),
            by_arn: true,
            expected_total: 0,
        },
        Step {
            label: "SET a total the largest Long dwarfs",
            body: json!({"operation": "SET", "quantity": 123}),
            by_arn: false,
            expected_total: 123,
        },
        Step {
            label: "REMOVE the largest Long from a smaller total saturates at zero",
            body: json!({"operation": "REMOVE", "quantity": i64::MAX}),
            by_arn: true,
            expected_total: 0,
        },
    ]
}

/// The rule this issue was filed about: an update applies the documented arithmetic to the
/// stored total, answers it, and a describe reads it back — for both documented
/// `resourceType`s, since the arithmetic is the resource's and not the type's.
#[tokio::test]
async fn an_update_applies_the_documented_arithmetic_and_reads_back() {
    for resource_type in ["REPLENISHABLE", "NON_REPLENISHABLE"] {
        let batch = Batch::new();
        let name = format!("arithmetic-{resource_type}");
        let expected_arn = arn(&format!("consumable-resource/{name}"));
        let (status, created) = create(
            &batch,
            json!({
                "consumableResourceName": &name,
                "resourceType": resource_type,
                "totalQuantity": STARTING_TOTAL,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {resource_type}: the resource the table walks is created, got {status} \
             {created}"
        );

        for case in steps() {
            let reference = if case.by_arn {
                expected_arn.clone()
            } else {
                name.clone()
            };
            let mut body = json!({"consumableResource": reference});
            for (member, value) in case.body.as_object().expect("the row's members") {
                body[member] = value.clone();
            }

            let (status, updated) = update(&batch, body).await;
            assert_eq!(
                status, 200,
                "case {resource_type}/{:?}: expected 200 from UpdateConsumableResource, \
                 got {status} {updated}",
                case.label
            );
            assert_eq!(
                updated["consumableResourceName"],
                json!(name),
                "case {resource_type}/{:?}: expected the stored name {name:?}, got {updated}",
                case.label
            );
            assert_eq!(
                updated["consumableResourceArn"],
                json!(expected_arn),
                "case {resource_type}/{:?}: expected the stored ARN {expected_arn:?}, got \
                 {updated}",
                case.label
            );
            assert_eq!(
                updated["totalQuantity"],
                json!(case.expected_total),
                "case {resource_type}/{:?}: expected totalQuantity {}, got {updated}",
                case.label,
                case.expected_total
            );

            // Persisted, not merely answered: the reader sees the same total, and the
            // derived availableQuantity follows it (nothing holds the resource).
            let (status, described) = describe(&batch, &name).await;
            assert_eq!(
                status, 200,
                "case {resource_type}/{:?}: expected 200 from DescribeConsumableResource, \
                 got {status} {described}",
                case.label
            );
            assert_eq!(
                described["totalQuantity"],
                json!(case.expected_total),
                "case {resource_type}/{:?}: expected the reader to report totalQuantity \
                 {}, got {described}",
                case.label,
                case.expected_total
            );
            assert_eq!(
                described["availableQuantity"],
                json!(case.expected_total),
                "case {resource_type}/{:?}: expected availableQuantity = totalQuantity - \
                 inUseQuantity = {}, got {described}",
                case.label,
                case.expected_total
            );
            assert_eq!(
                described["resourceType"],
                json!(resource_type),
                "case {resource_type}/{:?}: an update changes no member but the total, got \
                 {described}",
                case.label
            );
            assert_eq!(
                described["inUseQuantity"],
                json!(0),
                "case {resource_type}/{:?}: nothing holds the resource, so inUseQuantity \
                 stays 0, got {described}",
                case.label
            );
        }
    }
}

/// An update touches ONLY the resource it names: every other stored resource keeps the
/// quantity it had.
#[tokio::test]
async fn an_update_leaves_every_other_resource_alone() {
    let batch = Batch::new();
    let names = ["first", "second", "third"];
    for (index, name) in names.iter().enumerate() {
        let (status, created) = create(
            &batch,
            json!({"consumableResourceName": name, "totalQuantity": (index as i64 + 1) * 10}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name}: the resource is created, got {status} {created}"
        );
    }

    let (status, updated) = update(
        &batch,
        json!({"consumableResource": "second", "operation": "ADD", "quantity": 5}),
    )
    .await;
    assert_eq!(status, 200, "case second: expected 200, got {status} {updated}");

    for (index, name) in names.iter().enumerate() {
        let expected = if *name == "second" {
            25
        } else {
            (index as i64 + 1) * 10
        };
        let total = stored_total(&batch, name).await;
        assert_eq!(
            total, expected,
            "case {name}: expected totalQuantity {expected} after updating only \
             \"second\", got {total}"
        );
    }
}

/// The refusals, over the SET of things an update can get wrong — and the stored quantity
/// each one must leave untouched.
#[tokio::test]
async fn an_update_the_service_cannot_apply_is_refused_and_changes_nothing() {
    let batch = Batch::new();
    let (status, created) = create(
        &batch,
        json!({"consumableResourceName": "guarded", "totalQuantity": 10}),
    )
    .await;
    assert_eq!(status, 200, "the guarded resource is created, got {status} {created}");

    // (case, request body, the member or sentence the refusal must name)
    let refusals: Vec<(&str, Value, String)> = vec![
        (
            "an operation outside the documented three",
            json!({"consumableResource": "guarded", "operation": "SUBTRACT", "quantity": 1}),
            "operation".to_string(),
        ),
        (
            "a documented operation in the wrong case",
            json!({"consumableResource": "guarded", "operation": "add", "quantity": 1}),
            "operation".to_string(),
        ),
        (
            "an empty operation",
            json!({"consumableResource": "guarded", "operation": "", "quantity": 1}),
            "operation".to_string(),
        ),
        (
            "quantity one past its lower bound",
            json!({"consumableResource": "guarded", "operation": "SET", "quantity": -1}),
            "quantity".to_string(),
        ),
        (
            "quantity at the smallest Long",
            json!({"consumableResource": "guarded", "operation": "ADD", "quantity": i64::MIN}),
            "quantity".to_string(),
        ),
        (
            "a negative quantity with no operation",
            json!({"consumableResource": "guarded", "quantity": -5}),
            "quantity".to_string(),
        ),
        (
            "ADD past the largest Long",
            json!({"consumableResource": "guarded", "operation": "ADD", "quantity": i64::MAX}),
            "totalQuantity".to_string(),
        ),
        (
            "a name nothing was created under",
            json!({"consumableResource": "absent", "operation": "ADD", "quantity": 1}),
            format!("{} does not exist", arn("consumable-resource/absent")),
        ),
        (
            "an ARN nothing was created under",
            json!({
                "consumableResource": arn("consumable-resource/absent"),
                "operation": "SET",
                "quantity": 1,
            }),
            format!("{} does not exist", arn("consumable-resource/absent")),
        ),
        (
            "an identifier of another resource kind",
            json!({
                "consumableResource": arn("compute-environment/guarded"),
                "operation": "SET",
                "quantity": 1,
            }),
            format!("{} does not exist", arn("compute-environment/guarded")),
        ),
    ];

    for (case, body, named) in refusals {
        let (status, refused) = update(&batch, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ClientException",
            "case {case:?}: expected the modelled ClientException, got {refused}"
        );
        let message = error_message(&refused);
        assert!(
            message.contains(&named),
            "case {case:?}: expected the refusal to name {named:?}, got {message:?}"
        );
        let total = stored_total(&batch, "guarded").await;
        assert_eq!(
            total, 10,
            "case {case:?}: a refused update must leave the stored totalQuantity at 10, \
             got {total}"
        );
    }

    // Nothing was created for the identifiers that resolved to no resource.
    let (status, described) = describe(&batch, "absent").await;
    assert_eq!(
        status, 400,
        "case a refused update creates nothing: expected 400 for \"absent\", got {status} \
         {described}"
    );
}

/// RULE: an update carrying a `clientToken` is idempotent in that token — the identical
/// repeat succeeds with the same response and applies the arithmetic once — while an
/// update carrying no token is a new request every time.
#[tokio::test]
async fn a_client_token_makes_an_identical_repeat_the_same_request() {
    let batch = Batch::new();
    let (status, created) = create(
        &batch,
        json!({"consumableResourceName": "tokened", "totalQuantity": 0}),
    )
    .await;
    assert_eq!(status, 200, "the resource is created, got {status} {created}");

    // The rows vary what the repeated request asks for: each documented operation, with a
    // quantity, under its own token.
    let rows: Vec<(&str, Value, i64, i64)> = vec![
        // (case, the update's members, the total one application leaves, repeats)
        ("ADD under one token", json!({"operation": "ADD", "quantity": 5}), 5, 3),
        ("SET under one token", json!({"operation": "SET", "quantity": 40}), 40, 2),
        (
            "REMOVE under one token",
            json!({"operation": "REMOVE", "quantity": 15}),
            25,
            4,
        ),
        ("no operation, under one token", json!({"quantity": 9}), 9, 2),
    ];

    for (index, (case, members, expected_total, repeats)) in rows.into_iter().enumerate() {
        let token = format!("token-{index}");
        let mut body = json!({"consumableResource": "tokened", "clientToken": token});
        for (member, value) in members.as_object().expect("the row's members") {
            body[member] = value.clone();
        }

        let mut first: Option<Value> = None;
        for attempt in 1..=repeats {
            let (status, answered) = update(&batch, body.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?} (attempt {attempt}): every one of the identical requests \
                 succeeds, got {status} {answered}"
            );
            assert_eq!(
                answered["totalQuantity"],
                json!(expected_total),
                "case {case:?} (attempt {attempt}): expected the one application's \
                 totalQuantity {expected_total}, got {answered}"
            );
            match &first {
                None => first = Some(answered),
                Some(expected) => assert_eq!(
                    &answered, expected,
                    "case {case:?} (attempt {attempt}): the same request must answer the \
                     same response, got {answered}"
                ),
            }
        }
        let total = stored_total(&batch, "tokened").await;
        assert_eq!(
            total, expected_total,
            "case {case:?}: {repeats} identical requests must apply the update once, \
             leaving {expected_total}, got {total}"
        );
    }

    // The SAME token with a DIFFERENT payload is not that request: it is refused, and the
    // stored total stays where the token's own update left it.
    let (status, refused) = update(
        &batch,
        json!({
            "consumableResource": "tokened",
            "clientToken": "token-0",
            "operation": "ADD",
            "quantity": 6,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a token reused for a different payload: expected HTTP 400, got {status} \
         {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ClientException",
        "case a token reused for a different payload: expected the modelled \
         ClientException, got {refused}"
    );
    assert!(
        error_message(&refused).contains("clientToken"),
        "case a token reused for a different payload: expected the refusal to name \
         clientToken, got {refused}"
    );
    let total = stored_total(&batch, "tokened").await;
    assert_eq!(
        total, 9,
        "case a token reused for a different payload: the stored total is untouched, got \
         {total}"
    );

    // An update carrying NO token is a new request every time: two of them apply twice.
    for attempt in 1..=2 {
        let (status, answered) = update(
            &batch,
            json!({"consumableResource": "tokened", "operation": "ADD", "quantity": 4}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case no clientToken (attempt {attempt}): expected 200, got {status} {answered}"
        );
        assert_eq!(
            answered["totalQuantity"],
            json!(9 + 4 * attempt),
            "case no clientToken (attempt {attempt}): expected each untokened update to \
             apply, got {answered}"
        );
    }
    let total = stored_total(&batch, "tokened").await;
    assert_eq!(
        total, 17,
        "case no clientToken: two untokened ADDs of 4 apply twice, got {total}"
    );
}

/// The `operation` list the handler applies is the one the model documents: the constant
/// the service exposes and the list read off the member's doc agree, and every entry of it
/// is accepted.
#[tokio::test]
async fn every_documented_operation_is_accepted_and_no_other_is() {
    assert_eq!(
        CONSUMABLE_RESOURCE_UPDATE_OPERATIONS.to_vec(),
        DOCUMENTED_OPERATIONS.to_vec(),
        "the operations the service accepts must be the documented three, got \
         {CONSUMABLE_RESOURCE_UPDATE_OPERATIONS:?}"
    );
    let batch = Batch::new();
    for operation in DOCUMENTED_OPERATIONS {
        let name = format!("op-{operation}");
        let (status, created) = create(
            &batch,
            json!({"consumableResourceName": &name, "totalQuantity": 8}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {operation:?}: the resource is created, got {status} {created}"
        );
        let (status, updated) = update(
            &batch,
            json!({"consumableResource": &name, "operation": operation, "quantity": 3}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {operation:?}: a documented operation is accepted, got {status} {updated}"
        );
        let expected = match operation {
            "SET" => 3,
            "ADD" => 11,
            _ => 5,
        };
        let total = stored_total(&batch, &name).await;
        assert_eq!(
            total, expected,
            "case {operation:?}: expected totalQuantity {expected} after applying it to 8 \
             with quantity 3, got {total}"
        );
    }
}
