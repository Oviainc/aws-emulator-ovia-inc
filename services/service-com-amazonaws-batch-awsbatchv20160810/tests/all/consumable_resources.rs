// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for AWS Batch consumable resources, over the wire.
//!
//! Every case drives a real restJson1 request through the router the emulator mounts, so
//! what is asserted is the response a `batch` client gets.
//!
//! The rules under test, each over a SET of inputs:
//!
//! * `CreateConsumableResource` PERSISTS the resource it was asked for and answers the
//!   name it was given and the ARN it was stored under
//!   (`arn:aws:batch:<region>:<account>:consumable-resource/<name>`), and
//!   `DescribeConsumableResource` reads that record back by name and by ARN — the two
//!   forms `consumableResource` documents;
//! * the defaults and derived members the model documents: `resourceType` defaults to
//!   `REPLENISHABLE`, `inUseQuantity` is what the jobs holding the resource have taken
//!   (0 for one nothing has taken), `availableQuantity` is `totalQuantity -
//!   inUseQuantity`, and `createdAt` is the epoch-millisecond instant of the create;
//! * a create the service cannot accept is refused with the modelled `ClientException`
//!   (HTTP 400) and stores NOTHING: a name already stored ("Must be unique."), a negative
//!   `totalQuantity` ("Must be non-negative."), a `resourceType` outside the documented
//!   pair, and an empty name;
//! * a describe is a REPORTING call: two describes of the same resource answer
//!   identically, and a describe of a resource that was never created is refused rather
//!   than invented.

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state::{limits, CONSUMABLE_RESOURCE_TYPES};

use crate::job_lifecycle::{arn, error_code, error_message, Batch};

/// The `resourceType` values a consumable resource may carry, and which one a create that
/// names none gets. Source: the `CreateConsumableResourceRequest$resourceType` doc
/// (`smithy_query(query="doc", shape="CreateConsumableResourceRequest",
/// member="resourceType")`): "Can be one of: REPLENISHABLE (default),
/// NON_REPLENISHABLE".
const DOCUMENTED_RESOURCE_TYPES: [&str; 2] = ["REPLENISHABLE", "NON_REPLENISHABLE"];

/// The `resourceType` of a create that names none, from the same doc.
const DEFAULT_RESOURCE_TYPE: &str = "REPLENISHABLE";

async fn create(batch: &Batch, body: Value) -> (u16, Value) {
    batch.call("/v1/createconsumableresource", body).await
}

async fn describe(batch: &Batch, reference: &str) -> (u16, Value) {
    batch
        .call(
            "/v1/describeconsumableresource",
            json!({"consumableResource": reference}),
        )
        .await
}

/// One create request and what the resource it stores must read back as.
struct Case {
    /// What this row varies, named in every assertion.
    label: &'static str,
    name: &'static str,
    /// The request body members beyond `consumableResourceName`.
    extra: Value,
    expected_total: i64,
    expected_type: &'static str,
    expected_tags: Value,
}

/// The rows span what the rule generalises over: whether each optional member is given,
/// the published bounds of `totalQuantity` (a `Long`), both documented `resourceType`
/// values, the presence and size of `tags`, and the shape of the name.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "no optional member: resourceType and totalQuantity both defaulted",
            name: "plain",
            extra: json!({}),
            expected_total: 0,
            expected_type: DEFAULT_RESOURCE_TYPE,
            expected_tags: Value::Null,
        },
        Case {
            label: "totalQuantity at its lower bound, resourceType defaulted",
            name: "zero-quantity",
            extra: json!({"totalQuantity": 0}),
            expected_total: 0,
            expected_type: DEFAULT_RESOURCE_TYPE,
            expected_tags: Value::Null,
        },
        Case {
            label: "totalQuantity 1, resourceType named explicitly",
            name: "one_replenishable",
            extra: json!({"totalQuantity": 1, "resourceType": "REPLENISHABLE"}),
            expected_total: 1,
            expected_type: "REPLENISHABLE",
            expected_tags: Value::Null,
        },
        Case {
            label: "NON_REPLENISHABLE, the resourceType the issue did not cite",
            name: "NonReplenishable.1",
            extra: json!({"totalQuantity": 42, "resourceType": "NON_REPLENISHABLE"}),
            expected_total: 42,
            expected_type: "NON_REPLENISHABLE",
            expected_tags: Value::Null,
        },
        Case {
            label: "totalQuantity at the largest Long the model admits",
            name: "huge-quantity",
            extra: json!({"totalQuantity": i64::MAX}),
            expected_total: i64::MAX,
            expected_type: DEFAULT_RESOURCE_TYPE,
            expected_tags: Value::Null,
        },
        Case {
            label: "one tag",
            name: "one-tag",
            extra: json!({"totalQuantity": 7, "tags": {"team": "batch"}}),
            expected_total: 7,
            expected_type: DEFAULT_RESOURCE_TYPE,
            expected_tags: json!({"team": "batch"}),
        },
        Case {
            label: "three tags, one with an empty value",
            name: "three_tags",
            extra: json!({
                "totalQuantity": 1_000,
                "resourceType": "NON_REPLENISHABLE",
                "tags": {"team": "batch", "env": "test", "owner": ""},
            }),
            expected_total: 1_000,
            expected_type: "NON_REPLENISHABLE",
            expected_tags: json!({"team": "batch", "env": "test", "owner": ""}),
        },
    ]
}

/// The rule this issue was filed about: a create persists the resource and a describe
/// reads it back, by name and by ARN, with the documented defaults and derived
/// quantities.
#[tokio::test]
async fn a_created_consumable_resource_reads_back_by_name_and_by_arn() {
    let batch = Batch::new();
    for case in cases() {
        let mut body = json!({"consumableResourceName": case.name});
        for (member, value) in case.extra.as_object().expect("the row's extra members") {
            body[member] = value.clone();
        }

        let (status, created) = create(&batch, body).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from CreateConsumableResource, got {status} {created}",
            case.label
        );
        let expected_arn = arn(&format!("consumable-resource/{}", case.name));
        assert_eq!(
            created["consumableResourceName"],
            json!(case.name),
            "case {:?}: expected the created name {:?}, got {created}",
            case.label,
            case.name
        );
        assert_eq!(
            created["consumableResourceArn"],
            json!(expected_arn),
            "case {:?}: expected the ARN {expected_arn:?}, got {created}",
            case.label
        );

        // Read it back by NAME, and then by the ARN the create answered: both forms the
        // `consumableResource` member documents resolve to the same record.
        let (status, described) = describe(&batch, case.name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from DescribeConsumableResource, got {status} {described}",
            case.label
        );
        let (arn_status, by_arn) = describe(&batch, &expected_arn).await;
        assert_eq!(
            arn_status, 200,
            "case {:?}: expected 200 describing by ARN, got {arn_status} {by_arn}",
            case.label
        );
        assert_eq!(
            by_arn, described,
            "case {:?}: describing by ARN must answer exactly what describing by name \
             answered, got {by_arn} and {described}",
            case.label
        );

        assert_eq!(
            described["consumableResourceName"],
            json!(case.name),
            "case {:?}: expected the stored name {:?}, got {described}",
            case.label,
            case.name
        );
        assert_eq!(
            described["consumableResourceArn"],
            json!(expected_arn),
            "case {:?}: expected the stored ARN {expected_arn:?}, got {described}",
            case.label
        );
        assert_eq!(
            described["totalQuantity"],
            json!(case.expected_total),
            "case {:?}: expected totalQuantity {}, got {described}",
            case.label,
            case.expected_total
        );
        assert_eq!(
            described["inUseQuantity"],
            json!(0),
            "case {:?}: nothing holds a freshly created resource, so inUseQuantity must \
             be 0, got {described}",
            case.label
        );
        assert_eq!(
            described["availableQuantity"],
            json!(case.expected_total),
            "case {:?}: expected availableQuantity = totalQuantity - inUseQuantity = {}, \
             got {described}",
            case.label,
            case.expected_total
        );
        assert_eq!(
            described["resourceType"],
            json!(case.expected_type),
            "case {:?}: expected resourceType {:?}, got {described}",
            case.label,
            case.expected_type
        );
        assert_eq!(
            described["tags"],
            case.expected_tags,
            "case {:?}: expected the tags the create carried, got {described}",
            case.label
        );
        let created_at = described["createdAt"].as_i64().unwrap_or_default();
        assert!(
            created_at > 0,
            "case {:?}: expected createdAt in epoch milliseconds, got {described}",
            case.label
        );
        // Epoch MILLISECONDS, not seconds: anything after 2001-09-09 in seconds is below
        // this, so the unit is pinned rather than merely non-zero.
        assert!(
            created_at > 1_000_000_000_000,
            "case {:?}: expected createdAt in epoch milliseconds, got {created_at}",
            case.label
        );
    }
}

/// Every stored resource stays readable after the others are created: a create adds a
/// record, it does not replace the store.
#[tokio::test]
async fn creating_one_resource_leaves_every_earlier_one_readable() {
    let batch = Batch::new();
    let rows = cases();
    for case in &rows {
        let mut body = json!({"consumableResourceName": case.name});
        for (member, value) in case.extra.as_object().expect("the row's extra members") {
            body[member] = value.clone();
        }
        let (status, created) = create(&batch, body).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} {created}",
            case.label
        );
    }
    for case in &rows {
        let (status, described) = describe(&batch, case.name).await;
        assert_eq!(
            status, 200,
            "case {:?}: every created resource stays describable, got {status} {described}",
            case.label
        );
        assert_eq!(
            described["totalQuantity"],
            json!(case.expected_total),
            "case {:?}: expected the quantity this row was created with, got {described}",
            case.label
        );
    }
}

/// The refusals, over the SET of things a create can get wrong — and the record each one
/// must leave alone.
#[tokio::test]
async fn a_create_the_service_cannot_accept_is_refused_and_stores_nothing() {
    let batch = Batch::new();
    // The resource every "duplicate" row collides with.
    let (status, _) = create(
        &batch,
        json!({"consumableResourceName": "taken", "totalQuantity": 5}),
    )
    .await;
    assert_eq!(status, 200, "the resource the duplicate rows collide with is created");

    // (case, request body, the member the refusal must name)
    let refusals: Vec<(&str, Value, &str)> = vec![
        (
            "a name already stored",
            json!({"consumableResourceName": "taken", "totalQuantity": 99}),
            // A duplicate name is the one refusal that names no member: AWS answers the
            // bare `Object already exists` (`state::OBJECT_ALREADY_EXISTS`).
            "Object already exists",
        ),
        (
            "a name already stored, with no other member",
            json!({"consumableResourceName": "taken"}),
            "Object already exists",
        ),
        (
            "an empty name",
            json!({"consumableResourceName": ""}),
            "consumableResourceName",
        ),
        (
            "totalQuantity one past its lower bound",
            json!({"consumableResourceName": "negative-1", "totalQuantity": -1}),
            "totalQuantity",
        ),
        (
            "totalQuantity at the smallest Long",
            json!({"consumableResourceName": "negative-min", "totalQuantity": i64::MIN}),
            "totalQuantity",
        ),
        (
            "a resourceType outside the documented pair",
            json!({"consumableResourceName": "bad-type", "resourceType": "RECYCLABLE"}),
            "resourceType",
        ),
        (
            "a documented resourceType in the wrong case",
            json!({"consumableResourceName": "lower-type", "resourceType": "replenishable"}),
            "resourceType",
        ),
        (
            "an empty resourceType",
            json!({"consumableResourceName": "empty-type", "resourceType": ""}),
            "resourceType",
        ),
    ];

    for (case, body, named) in refusals {
        let requested = body["consumableResourceName"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (status, refused) = create(&batch, body).await;
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
            message.contains(named),
            "case {case:?}: expected the refusal to name {named:?}, got {message:?}"
        );
        if requested == "taken" {
            // A refused duplicate leaves the stored record exactly as it was.
            let (status, described) = describe(&batch, "taken").await;
            assert_eq!(
                status, 200,
                "case {case:?}: the stored resource survives a refused duplicate, got \
                 {status} {described}"
            );
            assert_eq!(
                described["totalQuantity"],
                json!(5),
                "case {case:?}: a refused duplicate must not overwrite the stored \
                 quantity, got {described}"
            );
        } else if !requested.is_empty() {
            // Nothing was stored under the refused name.
            let (status, described) = describe(&batch, &requested).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a refused create must store nothing under {requested:?}, \
                 got {status} {described}"
            );
        }
    }
}

/// A describe of a resource nothing created is refused, over every reference form, and a
/// describe changes nothing a later describe sees.
#[tokio::test]
async fn describing_an_unknown_resource_is_refused_and_a_describe_mutates_nothing() {
    let batch = Batch::new();
    let created = batch
        .ok(
            "/v1/createconsumableresource",
            json!({"consumableResourceName": "stable", "totalQuantity": 3}),
        )
        .await;
    let stored_arn = created["consumableResourceArn"]
        .as_str()
        .expect("the create answers an ARN")
        .to_string();

    for reference in [
        "absent",
        &arn("consumable-resource/absent"),
        &arn("compute-environment/stable"),
        "arn:aws:batch:us-east-1:000000000000:consumable-resource/",
        "stable:1",
    ] {
        let (status, refused) = describe(&batch, reference).await;
        assert_eq!(
            status, 400,
            "case {reference:?}: expected HTTP 400 for a resource nothing created, got \
             {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ClientException",
            "case {reference:?}: expected the modelled ClientException, got {refused}"
        );
    }

    // Two describes of the same resource answer identically: reading reports, it never
    // moves the resource on.
    let (_, first) = describe(&batch, "stable").await;
    let (_, second) = describe(&batch, &stored_arn).await;
    let (_, third) = describe(&batch, "stable").await;
    assert_eq!(
        first, second,
        "a describe by name and by ARN must answer the same record, got {first} and {second}"
    );
    assert_eq!(
        first, third,
        "a describe must not change what a later describe sees, got {first} then {third}"
    );
}

/// The consumable-resource store carries the maximum this emulator states
/// (`state::limits::MAX_CONSUMABLE_RESOURCES`), and a create past it answers the modelled
/// `ClientException` instead of growing the store.
#[tokio::test]
async fn the_consumable_resource_store_stops_at_its_stated_maximum() {
    let batch = Batch::new();
    for index in 0..limits::MAX_CONSUMABLE_RESOURCES {
        let (status, created) = create(
            &batch,
            json!({"consumableResourceName": format!("cr-{index}"), "totalQuantity": index}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {index}: a create inside the bound is accepted, got {status} {created}"
        );
    }
    let (status, refused) = create(
        &batch,
        json!({"consumableResourceName": "one-too-many", "totalQuantity": 1}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case one-too-many: expected HTTP 400 past the bound, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ClientException",
        "case one-too-many: expected the modelled ClientException, got {refused}"
    );
    let (status, described) = describe(&batch, "one-too-many").await;
    assert_eq!(
        status, 400,
        "case one-too-many: a refused create stores nothing, got {status} {described}"
    );
}

/// The `resourceType` list the handler applies is the one the model documents: the
/// constant the service exposes and the list read off the member's doc agree, and every
/// entry of it is accepted.
#[tokio::test]
async fn every_documented_resource_type_is_accepted_and_no_other_is() {
    assert_eq!(
        CONSUMABLE_RESOURCE_TYPES.to_vec(),
        DOCUMENTED_RESOURCE_TYPES.to_vec(),
        "the resourceType values the service accepts must be the documented pair, got \
         {CONSUMABLE_RESOURCE_TYPES:?}"
    );
    let batch = Batch::new();
    for resource_type in DOCUMENTED_RESOURCE_TYPES {
        let name = format!("type-{resource_type}");
        let (status, created) = create(
            &batch,
            json!({
                "consumableResourceName": name,
                "resourceType": resource_type,
                "totalQuantity": 2,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {resource_type:?}: a documented resourceType is accepted, got {status} \
             {created}"
        );
        let (status, described) = describe(&batch, &name).await;
        assert_eq!(
            status, 200,
            "case {resource_type:?}: expected 200, got {status} {described}"
        );
        assert_eq!(
            described["resourceType"],
            json!(resource_type),
            "case {resource_type:?}: expected the resourceType the create named, got \
             {described}"
        );
    }
}
