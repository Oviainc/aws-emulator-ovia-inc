// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `UpdateModelPackage`, and the `DescribeModelPackage` that
//! replays what it wrote.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over TABLES of inputs rather than one recorded call: that each
//! member the request carries is applied and every member it omits is left as it was;
//! that the approval status moves between every pair of values the modelled
//! `ModelApprovalStatus` enum carries; that `CustomerMetadataProperties` merges by key
//! while `CustomerMetadataPropertiesToRemove` drops only the keys it names (removals
//! before upserts, so a key named by both keeps the value the request set), up to and past
//! the modelled map bound; that `AdditionalInferenceSpecificationsToAdd` appends, refuses a
//! name the package already holds, and refuses to take the total past the documented 15;
//! that a repeated `ClientToken` replays instead of applying twice; and that an unknown
//! ARN, and an approval STATUS on an unversioned package, are refused.
//!
//! A versioned model package needs its group to exist first, so every versioned case
//! creates it through `CreateModelPackageGroup` over the same wire.
//!
//! The `Emulator` harness below is this module's own, as every other suite in this crate
//! keeps its own (`model_package_registry.rs`, `model_card_governance.rs`,
//! `studio_app_lifecycle.rs`, …): a suite's harness carries the resource helpers that
//! suite needs — here `versioned`, `unversioned` and `update` — and a shared one would
//! either accumulate every suite's or be wrapped per suite anyway. Only the dispatch
//! plumbing (`new`/`call_in`) is common, and that is four lines.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sagemaker_sagemaker::state;

use crate::service_resolved_members::{
    materialised_additional_inference_specifications, materialised_inference_specification,
};

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Long enough to pass `state::MODEL_PACKAGE_COMPLETED_AFTER`, so the registration
/// transition the clock owes has come due. Derived from that span so the wait can never
/// drift away from the window it has to outlast.
const PAST_COMPLETED_DUE: Duration =
    state::MODEL_PACKAGE_COMPLETED_AFTER.saturating_add(Duration::from_millis(300));

/// The group every versioned case registers its package into.
const GROUP: &str = "updatable";

/// Members `DescribeModelPackage` reports that an update is allowed to move even when the
/// request did not name them: the modification stamp it records, and the registration
/// status the clock settles on its own.
const NOT_COMPARED: &[&str] = &[
    "LastModifiedTime",
    "LastModifiedBy",
    "ModelPackageStatus",
    "ModelPackageStatusDetails",
];

/// A container the boundary validator accepts for an `InferenceSpecification`.
fn container(image: &str) -> Value {
    json!({"Image": image, "ModelDataUrl": "s3://models/artifact.tar.gz"})
}

/// An `AdditionalInferenceSpecificationDefinition` named `name`.
fn additional_spec(name: &str) -> Value {
    json!({
        "Name": name,
        "Containers": [container("123.dkr.ecr.us-east-1.amazonaws.com/neo:1")],
    })
}

/// `additional_spec(name)` as the registry STORES it: the service resolves each
/// container's image digest and checkpoint flag, so a read reports those beside the
/// members the request sent.
fn stored_spec(name: &str) -> Value {
    materialised_inference_specification(&additional_spec(name))
}

/// One emulator: the shared engine plus the SageMaker service mounted on the router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not SigV4; the request below carries a
        // credential scope only so the ARN region is the one the caller signed for.
        router.set_request_authentication(false);
        router.register_service(service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// Call `target` with a SigV4 credential scope naming `region`.
    async fn call_in(&self, region: &str, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/sagemaker/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(body).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    async fn call(&self, target: &str, body: &Value) -> (u16, Value) {
        self.call_in(state::DEFAULT_REGION, target, body).await
    }

    /// Call `target` in `region` and require a 200, returning the parsed output.
    async fn ok_in(&self, case: &str, region: &str, target: &str, body: &Value) -> Value {
        let (status, parsed) = self.call_in(region, target, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {target}, got {status} with {parsed}"
        );
        parsed
    }

    async fn ok(&self, case: &str, target: &str, body: &Value) -> Value {
        self.ok_in(case, state::DEFAULT_REGION, target, body).await
    }

    /// Create the model package group a versioned registration joins.
    async fn create_group(&self, case: &str, region: &str, name: &str) {
        self.ok_in(
            case,
            region,
            "CreateModelPackageGroup",
            &json!({"ModelPackageGroupName": name}),
        )
        .await;
    }

    /// Register a version inside [`GROUP`] (created here) in `region`, carrying `extras`,
    /// and return its ARN.
    async fn versioned(&self, case: &str, region: &str, extras: Value) -> String {
        self.create_group(case, region, GROUP).await;
        let body = body_with(&json!({"ModelPackageGroupName": GROUP}), &extras);
        let created = self
            .ok_in(case, region, "CreateModelPackage", &body)
            .await;
        str_field(&created, "ModelPackageArn")
    }

    /// Register an unversioned package under `name` and return its ARN.
    async fn unversioned(&self, case: &str, name: &str) -> String {
        let created = self
            .ok(case, "CreateModelPackage", &json!({"ModelPackageName": name}))
            .await;
        str_field(&created, "ModelPackageArn")
    }

    /// Update `arn` with `extras` and require a 200, returning the ARN it answered.
    async fn update(&self, case: &str, arn: &str, extras: &Value) -> String {
        let body = body_with(&json!({"ModelPackageArn": arn}), extras);
        let output = self.ok(case, "UpdateModelPackage", &body).await;
        str_field(&output, "ModelPackageArn")
    }

    /// Describe `identifier` (a name, a `name/version` pair or an ARN).
    async fn describe(&self, case: &str, identifier: &str) -> Value {
        self.ok(
            case,
            "DescribeModelPackage",
            &json!({"ModelPackageName": identifier}),
        )
        .await
    }
}

/// The `__type` an error body carries, with any `namespace#` prefix stripped.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn str_field(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string {member} in {body}"))
        .to_string()
}

/// The ARN form every model package carries:
/// `arn:aws:sagemaker:<region>:<account>:model-package/<name>[/<version>]`.
fn expected_arn(region: &str, resource: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:model-package/{resource}")
}

/// A request body: `members` plus whatever the row adds.
fn body_with(members: &Value, extras: &Value) -> Value {
    let mut body = members.as_object().cloned().unwrap_or_default();
    for (key, value) in extras.as_object().cloned().unwrap_or_default() {
        body.insert(key, value);
    }
    Value::Object(body)
}

/// Every member name either description carries.
fn member_names(before: &Value, after: &Value) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for described in [before, after] {
        for name in described.as_object().into_iter().flat_map(|body| body.keys()) {
            names.insert(name.clone());
        }
    }
    names
}

/// A `CustomerMetadataProperties` map of `count` distinct keys.
fn metadata_of(count: usize) -> Value {
    let map: serde_json::Map<String, Value> = (0..count)
        .map(|index| (format!("key{index}"), json!("value")))
        .collect();
    Value::Object(map)
}

/// One row of the "apply what is named, leave the rest" table: what the registration
/// carried, what the update carries, and the members `DescribeModelPackage` must report
/// differently afterwards.
struct Applied {
    case: &'static str,
    create_extras: Value,
    update_extras: Value,
    /// Describe members the update is expected to change, and their new values.
    expected: Value,
}

/// RULE: `UpdateModelPackage` applies every member the request carries to the model
/// package its `ModelPackageArn` resolves to — replacing a single-valued member, merging
/// the customer metadata map and appending the additional inference specifications — and
/// leaves every member the request omits exactly as it was, answering the package's own
/// ARN.
#[tokio::test]
async fn an_update_applies_the_members_it_names_and_leaves_the_rest_alone() {
    let rows = vec![
        Applied {
            case: "approval status alone, on a version awaiting manual approval",
            create_extras: json!({"ModelPackageDescription": "a detector",
                                  "Domain": "COMPUTER_VISION"}),
            update_extras: json!({"ModelApprovalStatus": "Approved"}),
            expected: json!({"ModelApprovalStatus": "Approved"}),
        },
        Applied {
            case: "approval status with its description",
            create_extras: json!({"ModelApprovalStatus": "Approved"}),
            update_extras: json!({"ModelApprovalStatus": "Rejected",
                                  "ApprovalDescription": "regression on the holdout set"}),
            expected: json!({"ModelApprovalStatus": "Rejected",
                             "ApprovalDescription": "regression on the holdout set"}),
        },
        Applied {
            case: "an approval description on its own leaves the status alone",
            create_extras: json!({"ModelApprovalStatus": "Approved"}),
            update_extras: json!({"ApprovalDescription": ""}),
            expected: json!({"ApprovalDescription": ""}),
        },
        Applied {
            case: "registration type promoted from Logged to Registered",
            create_extras: json!({"ModelPackageRegistrationType": "Logged"}),
            update_extras: json!({"ModelPackageRegistrationType": "Registered"}),
            expected: json!({"ModelPackageRegistrationType": "Registered"}),
        },
        Applied {
            case: "an inference specification replaces the stored one wholesale",
            create_extras: json!({
                "InferenceSpecification": {
                    "Containers": [container("123.dkr.ecr.us-east-1.amazonaws.com/a:1")],
                    "SupportedTransformInstanceTypes": ["ml.m5.large"],
                },
            }),
            update_extras: json!({
                "InferenceSpecification": {
                    "Containers": [container("123.dkr.ecr.us-east-1.amazonaws.com/b:2"),
                                   container("123.dkr.ecr.us-east-1.amazonaws.com/c:3")],
                    "SupportedContentTypes": ["application/json"],
                },
            }),
            expected: json!({
                "InferenceSpecification": {
                    "Containers": [container("123.dkr.ecr.us-east-1.amazonaws.com/b:2"),
                                   container("123.dkr.ecr.us-east-1.amazonaws.com/c:3")],
                    "SupportedContentTypes": ["application/json"],
                },
            }),
        },
        Applied {
            case: "a source URI set on a package registered without one",
            create_extras: json!({}),
            update_extras: json!({"SourceUri": "s3://models/source/détecteur.tar.gz"}),
            expected: json!({"SourceUri": "s3://models/source/détecteur.tar.gz"}),
        },
        Applied {
            case: "a model card replaces the stored card",
            create_extras: json!({
                "ModelCard": {"ModelCardContent": "{\"model_overview\":{\"model_id\":\"m-1\"}}",
                              "ModelCardStatus": "Draft"},
            }),
            update_extras: json!({
                "ModelCard": {"ModelCardContent": "{\"model_overview\":{\"model_id\":\"m-2\"}}",
                              "ModelCardStatus": "Approved"},
            }),
            expected: json!({
                "ModelCard": {"ModelCardContent": "{\"model_overview\":{\"model_id\":\"m-2\"}}",
                              "ModelCardStatus": "Approved"},
            }),
        },
        Applied {
            case: "a model life cycle stage moves on",
            create_extras: json!({
                "ModelLifeCycle": {"Stage": "Development", "StageStatus": "InProgress"},
            }),
            update_extras: json!({
                "ModelLifeCycle": {"Stage": "Production", "StageStatus": "Completed",
                                   "StageDescription": "serving live traffic"},
            }),
            expected: json!({
                "ModelLifeCycle": {"Stage": "Production", "StageStatus": "Completed",
                                   "StageDescription": "serving live traffic"},
            }),
        },
        Applied {
            case: "customer metadata merges with what the registration stored",
            create_extras: json!({"CustomerMetadataProperties": {"framework": "pytorch",
                                                                 "commit": "abc123"}}),
            update_extras: json!({"CustomerMetadataProperties": {"commit": "def456",
                                                                 "owner": "team-ml"}}),
            expected: json!({"CustomerMetadataProperties": {"framework": "pytorch",
                                                            "commit": "def456",
                                                            "owner": "team-ml"}}),
        },
        Applied {
            case: "additional inference specifications are appended after the stored ones",
            create_extras: json!({"AdditionalInferenceSpecifications": [additional_spec("neo")]}),
            update_extras: json!({
                "AdditionalInferenceSpecificationsToAdd": [additional_spec("graviton"),
                                                           additional_spec("inferentia")],
            }),
            expected: json!({
                "AdditionalInferenceSpecifications": [additional_spec("neo"),
                                                      additional_spec("graviton"),
                                                      additional_spec("inferentia")],
            }),
        },
        Applied {
            case: "several members at once",
            create_extras: json!({"ModelPackageDescription": "a detector"}),
            update_extras: json!({"ModelApprovalStatus": "Approved",
                                  "SourceUri": "s3://models/source/one.tar.gz",
                                  "CustomerMetadataProperties": {"owner": "team-ml"}}),
            expected: json!({"ModelApprovalStatus": "Approved",
                             "SourceUri": "s3://models/source/one.tar.gz",
                             "CustomerMetadataProperties": {"owner": "team-ml"}}),
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = Emulator::new();
        let arn = emulator
            .versioned(case, state::DEFAULT_REGION, row.create_extras)
            .await;
        // Past the registration transition the clock owes, so the comparison below is
        // not racing it.
        tokio::time::sleep(PAST_COMPLETED_DUE).await;
        let before = emulator.describe(case, &arn).await;

        let answered = emulator.update(case, &arn, &row.update_extras).await;
        assert_eq!(
            answered, arn,
            "case {case:?}: the update must answer the package's own ARN, got {answered:?}"
        );

        let after = emulator.describe(case, &arn).await;
        let expected = row
            .expected
            .as_object()
            .cloned()
            .expect("the row's expectations are an object");
        for (member, value) in &expected {
            // An inference specification is reported with the members the SERVICE
            // resolves for its containers laid over what the update sent.
            let value = match member.as_str() {
                "InferenceSpecification" => materialised_inference_specification(value),
                "AdditionalInferenceSpecifications" => {
                    materialised_additional_inference_specifications(value)
                }
                _ => value.clone(),
            };
            assert_eq!(
                after.get(member),
                Some(&value),
                "case {case:?}: expected {member} to be {value}, got {after}"
            );
        }
        for member in member_names(&before, &after) {
            if expected.contains_key(&member) || NOT_COMPARED.contains(&member.as_str()) {
                continue;
            }
            assert_eq!(
                before.get(&member),
                after.get(&member),
                "case {case:?}: {member} was not named by the update, so it must be \
                 unchanged; before {before}, after {after}"
            );
        }
        // A call modified the package, so the modification stamp is now reported.
        assert!(
            after.get("LastModifiedBy").is_some(),
            "case {case:?}: expected LastModifiedBy after an update, got {after}"
        );
        assert!(
            after.get("LastModifiedTime").is_some(),
            "case {case:?}: expected LastModifiedTime after an update, got {after}"
        );
    }
}

/// RULE: a model version's `ModelApprovalStatus` moves to whichever value the request
/// names, from whichever value it currently holds — the values being exactly those the
/// modelled enum carries (`smithy_query(query="shape", shape="ModelApprovalStatus")`:
/// `Approved`, `Rejected`, `PendingManualApproval`).
#[tokio::test]
async fn the_approval_status_moves_between_every_pair_of_modelled_values() {
    // The catalog is the modelled `ModelApprovalStatus` enum, in the model's own order.
    let statuses = ["Approved", "Rejected", "PendingManualApproval"];

    for from in statuses {
        for to in statuses {
            let case = format!("{from} -> {to}");
            let emulator = Emulator::new();
            let arn = emulator
                .versioned(
                    &case,
                    state::DEFAULT_REGION,
                    json!({"ModelApprovalStatus": from}),
                )
                .await;
            let registered = emulator.describe(&case, &arn).await;
            assert_eq!(
                registered.get("ModelApprovalStatus"),
                Some(&json!(from)),
                "case {case:?}: the registration must store the status it was given, \
                 got {registered}"
            );

            let description = format!("moved to {to}");
            let answered = emulator
                .update(
                    &case,
                    &arn,
                    &json!({"ModelApprovalStatus": to, "ApprovalDescription": description}),
                )
                .await;
            assert_eq!(
                answered, arn,
                "case {case:?}: the update must answer the package's own ARN, got {answered:?}"
            );

            let described = emulator.describe(&case, &arn).await;
            assert_eq!(
                described.get("ModelApprovalStatus"),
                Some(&json!(to)),
                "case {case:?}: expected ModelApprovalStatus {to}, got {described}"
            );
            assert_eq!(
                described.get("ApprovalDescription"),
                Some(&json!(description)),
                "case {case:?}: expected the approval description to be replayed, \
                 got {described}"
            );
        }
    }
}

/// RULE: the update answers — and `DescribeModelPackage` keeps reporting — the ARN built
/// from the region the registration was signed for, whatever region shapes a caller uses.
#[tokio::test]
async fn the_update_answers_the_arn_of_the_region_the_package_was_registered_in() {
    for region in ["us-east-1", "eu-central-1", "ap-southeast-2", "us-gov-west-1"] {
        let case = format!("region {region}");
        let emulator = Emulator::new();
        let arn = emulator.versioned(&case, region, json!({})).await;
        assert_eq!(
            arn,
            expected_arn(region, &format!("{GROUP}/1")),
            "case {case:?}: expected the registration ARN to name the signed region"
        );
        let answered = emulator
            .update(&case, &arn, &json!({"ModelApprovalStatus": "Approved"}))
            .await;
        assert_eq!(
            answered, arn,
            "case {case:?}: expected the update to answer {arn:?}, got {answered:?}"
        );
    }
}

/// RULE: `CustomerMetadataProperties` upserts the keys it names, `CustomerMetadataPropertiesToRemove`
/// drops the keys it names, every other stored key survives, and a package left with no
/// property at all reports no map (the modelled `CustomerMetadataMap` has a minimum length
/// of one).
#[tokio::test]
async fn customer_metadata_is_merged_and_removed_key_by_key() {
    // (case, what the registration stored, what the update sets, what it removes, what a
    // later describe must report — `Value::Null` for "no map at all")
    let rows: Vec<(&str, Value, Value, Value, Value)> = vec![
        (
            "a removal drops only the key it names",
            json!({"a": "1", "b": "2", "c": "3"}),
            Value::Null,
            json!(["b"]),
            json!({"a": "1", "c": "3"}),
        ),
        (
            "removing several keys at once",
            json!({"a": "1", "b": "2", "c": "3"}),
            Value::Null,
            json!(["a", "c"]),
            json!({"b": "2"}),
        ),
        (
            "removing a key the package never held is a no-op",
            json!({"a": "1"}),
            Value::Null,
            json!(["absent"]),
            json!({"a": "1"}),
        ),
        (
            "setting and removing in one call, on disjoint keys",
            json!({"a": "1", "b": "2"}),
            json!({"c": "3"}),
            json!(["a"]),
            json!({"b": "2", "c": "3"}),
        ),
        (
            "removing every key leaves no map at all",
            json!({"a": "1", "b": "2"}),
            Value::Null,
            json!(["a", "b"]),
            Value::Null,
        ),
        (
            "a value with the full punctuation the key and value patterns admit",
            json!({"a": "1"}),
            json!({"path.to/thing": "k=v+w-x@y:z"}),
            Value::Null,
            json!({"a": "1", "path.to/thing": "k=v+w-x@y:z"}),
        ),
    ];

    for (case, stored, set, remove, expected) in rows {
        let emulator = Emulator::new();
        let arn = emulator
            .versioned(case, state::DEFAULT_REGION, json!({"CustomerMetadataProperties": stored}))
            .await;

        let mut update = json!({});
        if !set.is_null() {
            update = body_with(&update, &json!({"CustomerMetadataProperties": set}));
        }
        if !remove.is_null() {
            update = body_with(&update, &json!({"CustomerMetadataPropertiesToRemove": remove}));
        }
        emulator.update(case, &arn, &update).await;

        let described = emulator.describe(case, &arn).await;
        let actual = described
            .get("CustomerMetadataProperties")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected CustomerMetadataProperties {expected}, got {actual}"
        );
    }
}

/// RULE: the removals are applied before the upserts, so a key named by BOTH
/// `CustomerMetadataPropertiesToRemove` and `CustomerMetadataProperties` keeps the value
/// the request set, and only a key named by the removal alone is dropped.
#[tokio::test]
async fn a_metadata_key_both_set_and_removed_keeps_the_value_the_request_set() {
    // (case, what the update sets, what it removes, what a later describe must report —
    //  the registration stored {"a": "1", "b": "1"})
    let rows: Vec<(&str, Value, Value, Value)> = vec![
        (
            "one key both set and removed",
            json!({"a": "2"}),
            json!(["a"]),
            json!({"a": "2", "b": "1"}),
        ),
        (
            "a key in both, beside a removal of another",
            json!({"a": "2", "c": "3"}),
            json!(["a", "b"]),
            json!({"a": "2", "c": "3"}),
        ),
        (
            "every key of the request is in both",
            json!({"a": "2", "b": "3"}),
            json!(["b", "a"]),
            json!({"a": "2", "b": "3"}),
        ),
    ];

    for (case, set, remove, expected) in rows {
        let emulator = Emulator::new();
        let arn = emulator
            .versioned(
                case,
                state::DEFAULT_REGION,
                json!({"CustomerMetadataProperties": {"a": "1", "b": "1"}}),
            )
            .await;
        emulator
            .update(
                case,
                &arn,
                &json!({
                    "CustomerMetadataProperties": set,
                    "CustomerMetadataPropertiesToRemove": remove,
                }),
            )
            .await;
        let described = emulator.describe(case, &arn).await;
        assert_eq!(
            described.get("CustomerMetadataProperties"),
            Some(&expected),
            "case {case:?}: expected CustomerMetadataProperties {expected}, got {described}"
        );
    }
}

/// RULE: a model package holds at most the modelled maximum of 50 customer metadata
/// properties (`CustomerMetadataMap`'s `length 1..50`), counting what it already holds plus
/// what the update sets; an update that would take it past that is refused 400
/// `ValidationException`.
#[tokio::test]
async fn an_update_cannot_take_the_metadata_map_past_its_modelled_bound() {
    // (case, properties the registration stored, how many new keys the update sets,
    //  whether it is accepted)
    let rows = [
        ("one property, far below the bound", 1_usize, 1_usize, true),
        ("exactly at the bound after the update", 49, 1, true),
        ("already at the bound, re-setting a held key", 50, 0, true),
        ("one past the bound", 50, 1, false),
        ("far past the bound", 49, 5, false),
    ];

    for (case, stored, added, accepted) in rows {
        let emulator = Emulator::new();
        let arn = emulator
            .versioned(
                case,
                state::DEFAULT_REGION,
                json!({"CustomerMetadataProperties": metadata_of(stored)}),
            )
            .await;
        // `added` keys the package does not hold yet, plus (when the row adds none) one it
        // does, so the row that is already at the bound still carries a map.
        let set: serde_json::Map<String, Value> = if added == 0 {
            [("key0".to_string(), json!("updated"))].into_iter().collect()
        } else {
            (stored..stored + added)
                .map(|index| (format!("key{index}"), json!("value")))
                .collect()
        };
        let (status, body) = emulator
            .call(
                "UpdateModelPackage",
                &json!({
                    "ModelPackageArn": arn,
                    "CustomerMetadataProperties": Value::Object(set),
                }),
            )
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for {} + {added} properties, got {status} \
                 with {body}",
                stored
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for {} + {added} properties, got {status} \
                 with {body}",
                stored
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
        }
    }
}

/// RULE: an additional inference specification's name "must be unique within the list of
/// your additional inference specifications for a particular model package"
/// (`AdditionalInferenceSpecificationDefinition$Name`'s model doc), so an addition that
/// repeats a name — one the package holds or one the same request already added — is
/// refused 400 `ConflictException`, and the stored list is left untouched.
#[tokio::test]
async fn adding_an_additional_inference_specification_name_twice_conflicts() {
    let rows: Vec<(&str, Value)> = vec![
        (
            "a name the package already holds",
            json!([additional_spec("neo")]),
        ),
        (
            "a held name behind a fresh one",
            json!([additional_spec("graviton"), additional_spec("neo")]),
        ),
        (
            "a name repeated within one request",
            json!([additional_spec("graviton"), additional_spec("graviton")]),
        ),
    ];

    for (case, to_add) in rows {
        let emulator = Emulator::new();
        let arn = emulator
            .versioned(
                case,
                state::DEFAULT_REGION,
                json!({"AdditionalInferenceSpecifications": [additional_spec("neo")]}),
            )
            .await;
        let (status, refused) = emulator
            .call(
                "UpdateModelPackage",
                &json!({
                    "ModelPackageArn": arn,
                    "AdditionalInferenceSpecificationsToAdd": to_add,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refused}"
        );
        let described = emulator.describe(case, &arn).await;
        assert_eq!(
            described.get("AdditionalInferenceSpecifications"),
            Some(&json!([stored_spec("neo")])),
            "case {case:?}: a refused addition must leave the stored list untouched, \
             got {described}"
        );
    }
}

/// RULE: "Total number of additional Inference Specifications can not exceed 15"
/// (`UpdateModelPackageInput$AdditionalInferenceSpecificationsToAdd`'s model doc): the
/// package's own list plus the additions, so an update that would take the total past 15 is
/// refused 400 `ValidationException`.
#[tokio::test]
async fn additional_inference_specifications_stop_at_the_documented_total() {
    // (case, how many the registration carried, how many the update adds, accepted)
    let rows = [
        ("a couple, far below the total", 2_usize, 3_usize, true),
        ("exactly at the total after the update", 14, 1, true),
        ("filled to the total in one update", 1, 14, true),
        ("one past the total", 15, 1, false),
        ("several past the total", 14, 3, false),
    ];

    for (case, stored, added, accepted) in rows {
        let emulator = Emulator::new();
        let held: Vec<Value> = (0..stored)
            .map(|index| additional_spec(&format!("held{index}")))
            .collect();
        let arn = emulator
            .versioned(
                case,
                state::DEFAULT_REGION,
                json!({"AdditionalInferenceSpecifications": held}),
            )
            .await;
        let to_add: Vec<Value> = (0..added)
            .map(|index| additional_spec(&format!("added{index}")))
            .collect();
        let (status, body) = emulator
            .call(
                "UpdateModelPackage",
                &json!({
                    "ModelPackageArn": arn,
                    "AdditionalInferenceSpecificationsToAdd": to_add,
                }),
            )
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for {stored} + {added}, got {status} \
                 with {body}"
            );
            let described = emulator.describe(case, &arn).await;
            let total = described
                .get("AdditionalInferenceSpecifications")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default();
            assert_eq!(
                total,
                stored + added,
                "case {case:?}: expected {} specifications after the update, got {described}",
                stored + added
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for {stored} + {added}, got {status} \
                 with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
        }
    }
}

/// RULE: `ClientToken` "guarantees that the call to this API is idempotent", so a retry
/// presenting a token already applied to the package answers the same ARN and applies
/// nothing again — while a token the package has not seen applies normally.
#[tokio::test]
async fn a_repeated_client_token_replays_instead_of_applying_the_update_twice() {
    let case = "retried addition";
    let emulator = Emulator::new();
    let arn = emulator.versioned(case, state::DEFAULT_REGION, json!({})).await;

    let add_neo = json!({
        "ClientToken": "token-one",
        "AdditionalInferenceSpecificationsToAdd": [additional_spec("neo")],
        "ModelApprovalStatus": "Approved",
    });
    emulator.update(case, &arn, &add_neo).await;
    // The very same call again: without the token this would be refused as a repeated
    // name, and with it the list is left exactly as the first call left it.
    let answered = emulator.update(case, &arn, &add_neo).await;
    assert_eq!(
        answered, arn,
        "case {case:?}: a replayed update must answer the package's ARN, got {answered:?}"
    );
    let described = emulator.describe(case, &arn).await;
    assert_eq!(
        described.get("AdditionalInferenceSpecifications"),
        Some(&json!([stored_spec("neo")])),
        "case {case:?}: the retry must not append a second copy, got {described}"
    );

    // A token the package has not seen is a new call, and applies.
    emulator
        .update(
            case,
            &arn,
            &json!({
                "ClientToken": "token-two",
                "AdditionalInferenceSpecificationsToAdd": [additional_spec("graviton")],
            }),
        )
        .await;
    let described = emulator.describe(case, &arn).await;
    assert_eq!(
        described.get("AdditionalInferenceSpecifications"),
        Some(&json!([stored_spec("neo"), stored_spec("graviton")])),
        "case {case:?}: a fresh token must apply its addition, got {described}"
    );

    // A removal is not re-applied either: the key set between the first call and the
    // retry survives it.
    let remove = json!({"ClientToken": "token-three",
                        "CustomerMetadataPropertiesToRemove": ["owner"]});
    emulator
        .update(case, &arn, &json!({"CustomerMetadataProperties": {"owner": "team-ml"}}))
        .await;
    emulator.update(case, &arn, &remove).await;
    emulator
        .update(case, &arn, &json!({"CustomerMetadataProperties": {"owner": "team-ops"}}))
        .await;
    emulator.update(case, &arn, &remove).await;
    let described = emulator.describe(case, &arn).await;
    assert_eq!(
        described.get("CustomerMetadataProperties"),
        Some(&json!({"owner": "team-ops"})),
        "case {case:?}: a replayed removal must not drop the key again, got {described}"
    );

    // A token is remembered whether or not the call it arrived on changed anything, so
    // "already applied" does not depend on that: this first call sets the metadata key to
    // the value it already holds, and the retry of the same token is still a replay.
    emulator
        .update(
            case,
            &arn,
            &json!({"ClientToken": "token-four",
                    "CustomerMetadataProperties": {"owner": "team-ops"}}),
        )
        .await;
    emulator
        .update(
            case,
            &arn,
            &json!({
                "ClientToken": "token-four",
                "AdditionalInferenceSpecificationsToAdd": [additional_spec("trainium")],
            }),
        )
        .await;
    let described = emulator.describe(case, &arn).await;
    assert_eq!(
        described.get("AdditionalInferenceSpecifications"),
        Some(&json!([stored_spec("neo"), stored_spec("graviton")])),
        "case {case:?}: a token already seen on a no-op call must still replay, \
         got {described}"
    );
}

/// RULE: `UpdateModelPackage` answers only for an ARN some stored model package carries;
/// anything else is refused 400 `ValidationException` quoting the ARN the request passed.
/// An ARN resolves to a package by its resource part, exactly as `DescribeModelPackage`
/// resolves a `VersionedArnOrName`, so the two agree on which ARNs name a package.
#[tokio::test]
async fn updating_an_unknown_model_package_is_refused() {
    let emulator = Emulator::new();
    emulator
        .versioned("a package to miss", state::DEFAULT_REGION, json!({}))
        .await;

    let rows = [
        (
            "a group with no such version",
            expected_arn(state::DEFAULT_REGION, &format!("{GROUP}/9")),
        ),
        (
            "a name never registered",
            expected_arn(state::DEFAULT_REGION, "absent"),
        ),
    ];

    for (case, arn) in rows {
        let (status, refused) = emulator
            .call(
                "UpdateModelPackage",
                &json!({"ModelPackageArn": arn, "ModelApprovalStatus": "Approved"}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused}"
        );
        assert!(
            message(&refused).contains(&arn),
            "case {case:?}: expected the message to quote {arn:?}, got {:?}",
            message(&refused)
        );
    }
}

/// RULE: an approval status belongs to a model VERSION inside a model package group —
/// `ModelApprovalStatus` "does not apply to unversioned models" — so a request moving one
/// on an unversioned package is refused 400 `ValidationException` rather than answered
/// with a success that changed nothing. The rule is exactly that member: see
/// [`an_unversioned_package_takes_the_members_that_do_apply_to_it`] for
/// `ApprovalDescription`, which carries no such statement.
#[tokio::test]
async fn an_approval_status_on_an_unversioned_package_is_refused() {
    let rows: Vec<(&str, Value)> = vec![
        ("a status alone", json!({"ModelApprovalStatus": "Approved"})),
        (
            "a status with its description",
            json!({"ModelApprovalStatus": "Approved", "ApprovalDescription": "looks fine"}),
        ),
        (
            "a status beside a member that does apply",
            json!({"ModelApprovalStatus": "Rejected", "ApprovalDescription": "no",
                   "SourceUri": "s3://models/source/one.tar.gz"}),
        ),
    ];

    for (index, (case, extras)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("unversioned{index}");
        let arn = emulator.unversioned(case, &name).await;
        let body = body_with(&json!({"ModelPackageArn": arn}), &extras);
        let (status, refused) = emulator.call("UpdateModelPackage", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused}"
        );
        // Nothing of the refused request was applied, not even the member that does apply.
        let described = emulator.describe(case, &name).await;
        assert_eq!(
            described.get("SourceUri"),
            None,
            "case {case:?}: a refused update must apply none of its members, got {described}"
        );
        assert_eq!(
            described.get("ModelApprovalStatus"),
            None,
            "case {case:?}: an unversioned package carries no approval status, \
             got {described}"
        );

        // The refusal is a property of the package, not of what has been applied to it, so
        // it is the same answer however often the request is presented — including a retry
        // carrying the idempotency token of a call that was refused.
        let retried = body_with(&body, &json!({"ClientToken": "token-retry"}));
        for attempt in 1..=2 {
            let (status, refused) = emulator.call("UpdateModelPackage", &retried).await;
            assert_eq!(
                status, 400,
                "case {case:?}: attempt {attempt} of the tokened request must still be \
                 refused, got {status} with {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ValidationException",
                "case {case:?}: attempt {attempt} expected ValidationException, got {refused}"
            );
        }
    }
}

/// RULE: an unversioned model package accepts every member that does apply to it, and the
/// update is replayed by `DescribeModelPackage` the same way a version's is.
#[tokio::test]
async fn an_unversioned_package_takes_the_members_that_do_apply_to_it() {
    let case = "unversioned source URI and metadata";
    let emulator = Emulator::new();
    let name = "standalone";
    let arn = emulator.unversioned(case, name).await;
    let answered = emulator
        .update(
            case,
            &arn,
            &json!({"SourceUri": "s3://models/source/one.tar.gz",
                    "CustomerMetadataProperties": {"owner": "team-ml"},
                    "ModelPackageRegistrationType": "Registered",
                    "ApprovalDescription": "reviewed off-registry"}),
        )
        .await;
    assert_eq!(
        answered, arn,
        "case {case:?}: expected the update to answer {arn:?}, got {answered:?}"
    );
    let described = emulator.describe(case, name).await;
    for (member, value) in [
        ("SourceUri", json!("s3://models/source/one.tar.gz")),
        ("CustomerMetadataProperties", json!({"owner": "team-ml"})),
        ("ModelPackageRegistrationType", json!("Registered")),
        // Only `ModelApprovalStatus` is documented as not applying to an unversioned
        // package; the free-text description is stored and replayed like any other member.
        ("ApprovalDescription", json!("reviewed off-registry")),
    ] {
        assert_eq!(
            described.get(member),
            Some(&value),
            "case {case:?}: expected {member} to be {value}, got {described}"
        );
    }
}

/// RULE: an update that changes nothing modifies nothing — a request naming no member, or
/// one whose members already hold the values it asks for, reports no modification that did
/// not happen — while still answering the package's ARN.
#[tokio::test]
async fn an_update_that_changes_nothing_reports_no_modification() {
    // (case, what the registration stored, the update that changes nothing)
    let rows: Vec<(&str, Value, Value)> = vec![
        ("no member at all", json!({}), json!({})),
        (
            "an approval status the version already holds",
            json!({"ModelApprovalStatus": "Approved"}),
            json!({"ModelApprovalStatus": "Approved"}),
        ),
        (
            "a registration type the package already holds",
            json!({"ModelPackageRegistrationType": "Logged"}),
            json!({"ModelPackageRegistrationType": "Logged"}),
        ),
        (
            "a metadata key set to the value it already has",
            json!({"CustomerMetadataProperties": {"owner": "team-ml"}}),
            json!({"CustomerMetadataProperties": {"owner": "team-ml"}}),
        ),
        (
            "a removal naming only keys the package never held",
            json!({"CustomerMetadataProperties": {"owner": "team-ml"}}),
            json!({"CustomerMetadataPropertiesToRemove": ["absent", "gone"]}),
        ),
        (
            // `AdditionalInferenceSpecificationsToAdd` is modelled `length 1..15`, so an
            // empty list is refused at the boundary; `CustomerMetadataKeyList` carries no
            // length constraint, so an empty removal reaches the handler.
            "an empty list of removals",
            json!({}),
            json!({"CustomerMetadataPropertiesToRemove": []}),
        ),
    ];

    for (case, create_extras, update_extras) in rows {
        let emulator = Emulator::new();
        let arn = emulator
            .versioned(
                case,
                state::DEFAULT_REGION,
                body_with(&json!({"ModelPackageDescription": "a detector"}), &create_extras),
            )
            .await;
        // Past the registration transition, so the only write the clock owes is already
        // done and the description below is stable.
        tokio::time::sleep(PAST_COMPLETED_DUE).await;
        let before = emulator.describe(case, &arn).await;

        let answered = emulator.update(case, &arn, &update_extras).await;
        assert_eq!(
            answered, arn,
            "case {case:?}: expected the update to answer {arn:?}, got {answered:?}"
        );

        let after = emulator.describe(case, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: an update that changes nothing must change nothing; \
             before {before}, after {after}"
        );
        assert_eq!(
            after.get("LastModifiedBy"),
            None,
            "case {case:?}: no call has modified the package, so no modifying identity is \
             reported; got {after}"
        );
    }
}
