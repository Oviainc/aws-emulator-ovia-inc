// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `UpdateArtifact`: the call that edits a stored ML Lineage
//! artifact, and the refusal it gives for an `ArtifactArn` the account does not hold.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the
//! members an update applies (name, `Properties` replacing the stored map, then
//! `PropertiesToRemove` pruning it) read back through `DescribeArtifact`; the
//! `LastModifiedTime` a mutation moves; the modelled `ResourceNotFound` for an ARN
//! nothing holds, a deleted artifact's among them; and the refusals that leave the
//! stored artifact exactly as it was.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

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
        router.register_service(
            service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(engine),
        );
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
        self.call_in("us-east-1", target, body).await
    }

    /// Call `target` in `region` and require a 200, returning the parsed output.
    async fn ok_in(&self, region: &str, target: &str, body: &Value) -> Value {
        let (status, parsed) = self.call_in(region, target, body).await;
        assert_eq!(
            status, 200,
            "case {target} in {region}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    async fn ok(&self, target: &str, body: &Value) -> Value {
        self.ok_in("us-east-1", target, body).await
    }

    /// Create one artifact in `region` and answer the `ArtifactArn` it was minted under.
    async fn create_artifact_in(
        &self,
        region: &str,
        name: &str,
        properties: Option<Value>,
    ) -> String {
        let mut body = Map::new();
        body.insert("ArtifactName".to_string(), json!(name));
        body.insert(
            "Source".to_string(),
            json!({ "SourceUri": format!("s3://bucket/{name}.csv") }),
        );
        body.insert("ArtifactType".to_string(), json!("DataSet"));
        if let Some(properties) = properties {
            body.insert("Properties".to_string(), properties);
        }
        let created = self
            .ok_in(region, "CreateArtifact", &Value::Object(body))
            .await;
        str_field(&created, "ArtifactArn")
    }

    async fn create_artifact(&self, name: &str, properties: Option<Value>) -> String {
        self.create_artifact_in("us-east-1", name, properties).await
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
        .unwrap_or_else(|| panic!("expected a string member {member} in {body}"))
        .to_string()
}

fn f64_field(case: &str, body: &Value, member: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} in {body}"))
}

/// A name of exactly `len` characters inside the modelled `ExperimentEntityName` pattern
/// (`^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$`), so a case can sit on the published maximum of
/// 120 (`smithy_query(query="operation",operation="UpdateArtifact")`:
/// `ArtifactName` len 1..120).
fn name_of_len(len: usize) -> String {
    "n".repeat(len)
}

/// A `Properties` map of `count` entries, for a case that differs along the ENTRY COUNT.
fn properties_of(count: usize) -> Value {
    let map: Map<String, Value> = (0..count)
        .map(|index| (format!("key{index}"), json!(format!("value{index}"))))
        .collect();
    Value::Object(map)
}

/// The sentence this service refuses a lineage artifact it does not hold with — the one
/// `DescribeArtifact` and `DeleteArtifact` already give for the same fact.
fn not_found_message(arn: &str) -> String {
    format!("Artifact '{arn}' does not exist.")
}

/// One row of the apply table: what the artifact was created with, the update sent to it,
/// and what a read must report afterwards.
struct Applied {
    case: &'static str,
    created_properties: Option<Value>,
    /// The update body, without the `ArtifactArn` the test fills in.
    update: Value,
    /// The name the artifact answers to after the update.
    expected_name: String,
    /// `Properties` as a read must report it — the map the artifact HOLDS, so the empty
    /// map where it holds nothing.
    expected_properties: Value,
}

#[tokio::test]
async fn update_artifact_applies_the_members_the_request_carries_and_leaves_the_rest() {
    let emulator = Emulator::new();

    let long_name = name_of_len(120);
    // Rows differ along what the rule generalises over: which members the update carries,
    // the shape of the property map it sends (absent, empty, replacing, pruned, 30 entries
    // — the modelled maximum), whether a pruned key is even present, and the length of
    // the new name.
    let rows = vec![
        Applied {
            case: "rename-only-keeps-properties",
            created_properties: Some(json!({ "stage": "raw" })),
            update: json!({ "ArtifactName": "renamed-artifact" }),
            expected_name: "renamed-artifact".to_string(),
            expected_properties: json!({ "stage": "raw" }),
        },
        Applied {
            case: "rename-to-the-published-maximum-length",
            created_properties: None,
            update: json!({ "ArtifactName": long_name.clone() }),
            expected_name: long_name.clone(),
            expected_properties: json!({}),
        },
        Applied {
            case: "properties-overwrite-rather-than-merge",
            created_properties: Some(json!({ "stage": "raw", "keep": "yes" })),
            update: json!({ "Properties": { "stage": "curated" } }),
            expected_name: "properties-overwrite-rather-than-merge".to_string(),
            expected_properties: json!({ "stage": "curated" }),
        },
        Applied {
            case: "properties-to-remove-prunes-a-subset",
            created_properties: Some(json!({ "a": "1", "b": "2", "c": "3" })),
            update: json!({ "PropertiesToRemove": ["b"] }),
            expected_name: "properties-to-remove-prunes-a-subset".to_string(),
            expected_properties: json!({ "a": "1", "c": "3" }),
        },
        Applied {
            case: "properties-to-remove-applies-to-the-new-map",
            created_properties: Some(json!({ "a": "1" })),
            update: json!({ "Properties": { "x": "9", "y": "8" }, "PropertiesToRemove": ["x"] }),
            expected_name: "properties-to-remove-applies-to-the-new-map".to_string(),
            expected_properties: json!({ "y": "8" }),
        },
        Applied {
            case: "properties-to-remove-of-a-key-not-present",
            created_properties: Some(json!({ "a": "1" })),
            update: json!({ "PropertiesToRemove": ["absent", "also-absent"] }),
            expected_name: "properties-to-remove-of-a-key-not-present".to_string(),
            expected_properties: json!({ "a": "1" }),
        },
        Applied {
            case: "properties-to-remove-against-an-artifact-holding-none",
            created_properties: None,
            update: json!({ "PropertiesToRemove": ["a"] }),
            expected_name: "properties-to-remove-against-an-artifact-holding-none".to_string(),
            expected_properties: json!({}),
        },
        Applied {
            case: "properties-replaced-by-the-empty-map",
            created_properties: Some(json!({ "a": "1" })),
            update: json!({ "Properties": {} }),
            expected_name: "properties-replaced-by-the-empty-map".to_string(),
            expected_properties: json!({}),
        },
        Applied {
            case: "properties-at-the-modelled-maximum-entry-count",
            created_properties: Some(json!({ "a": "1" })),
            update: json!({ "Properties": properties_of(30) }),
            expected_name: "properties-at-the-modelled-maximum-entry-count".to_string(),
            expected_properties: properties_of(30),
        },
        Applied {
            case: "name-and-properties-together",
            created_properties: Some(json!({ "a": "1" })),
            update: json!({ "ArtifactName": "renamed-and-repropertied", "Properties": { "b": "2" } }),
            expected_name: "renamed-and-repropertied".to_string(),
            expected_properties: json!({ "b": "2" }),
        },
        Applied {
            case: "no-member-beyond-the-arn",
            created_properties: Some(json!({ "a": "1" })),
            update: json!({}),
            expected_name: "no-member-beyond-the-arn".to_string(),
            expected_properties: json!({ "a": "1" }),
        },
    ];

    for row in &rows {
        let case = row.case;
        let arn = emulator
            .create_artifact(case, row.created_properties.clone())
            .await;
        let before = emulator
            .ok("DescribeArtifact", &json!({ "ArtifactArn": arn }))
            .await;
        let created_at = f64_field(case, &before, "CreationTime");
        let modified_before = f64_field(case, &before, "LastModifiedTime");

        let mut update = row
            .update
            .as_object()
            .expect("every row's update body is an object")
            .clone();
        update.insert("ArtifactArn".to_string(), json!(arn));
        let (status, updated) = emulator.call("UpdateArtifact", &Value::Object(update)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from UpdateArtifact, got {status} with {updated}"
        );
        assert_eq!(
            updated.get("ArtifactArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: the output carries the artifact's own ARN, got {updated:?}"
        );
        assert_eq!(
            updated.as_object().map(Map::len),
            Some(1),
            "case {case:?}: the output models ArtifactArn alone, got {updated:?}"
        );

        // Round trip: the update is read back through the implemented reader.
        let described = emulator
            .ok("DescribeArtifact", &json!({ "ArtifactArn": arn }))
            .await;
        assert_eq!(
            described.get("ArtifactName").and_then(Value::as_str),
            Some(row.expected_name.as_str()),
            "case {case:?}: ArtifactName after the update, got {described:?}"
        );
        assert_eq!(
            described.get("Properties"),
            Some(&row.expected_properties),
            "case {case:?}: Properties after the update, got {described:?}"
        );
        // Nothing the update did not name moves: the source — with the `SourceTypes` the
        // artifact holds — the type and the creation instant are the create's.
        assert_eq!(
            described.get("Source"),
            Some(&json!({
                "SourceUri": format!("s3://bucket/{case}.csv"),
                "SourceTypes": [],
            })),
            "case {case:?}: Source is the create's, got {described:?}"
        );
        assert_eq!(
            described.get("ArtifactType").and_then(Value::as_str),
            Some("DataSet"),
            "case {case:?}: ArtifactType is the create's, got {described:?}"
        );
        assert_eq!(
            f64_field(case, &described, "CreationTime"),
            created_at,
            "case {case:?}: CreationTime never moves, got {described:?}"
        );
        // A mutation moves `LastModifiedTime`, whatever the request changed.
        let modified_after = f64_field(case, &described, "LastModifiedTime");
        assert!(
            modified_after >= modified_before && modified_after >= created_at,
            "case {case:?}: LastModifiedTime must not go backwards, got {modified_after} \
             after {modified_before} (created {created_at})"
        );
    }
}

#[tokio::test]
async fn updating_an_artifact_arn_nothing_holds_is_refused_with_resource_not_found() {
    let emulator = Emulator::new();

    // One artifact that is created and then deleted: its ARN is inside the modelled
    // pattern and resolves to nothing, which is the case a lineage client uses to learn
    // that an entity is gone.
    let deleted = emulator.create_artifact("deleted-artifact", None).await;
    emulator
        .ok("DeleteArtifact", &json!({ "ArtifactArn": deleted }))
        .await;
    // One artifact that stays, so the refusals below are read against a live store.
    let present = emulator
        .create_artifact("present-artifact", Some(json!({ "stage": "raw" })))
        .await;

    // Rows differ along WHY the ARN resolves to nothing (deleted, never created, another
    // Region, another partition) and along which members the update carries.
    let cases: Vec<(&str, String, Value)> = vec![
        (
            "deleted-artifact-renamed",
            deleted.clone(),
            json!({ "ArtifactName": "a-new-name" }),
        ),
        (
            "deleted-artifact-repropertied",
            deleted.clone(),
            json!({ "Properties": { "probe": "post-delete" } }),
        ),
        (
            "deleted-artifact-pruned",
            deleted.clone(),
            json!({ "PropertiesToRemove": ["stage"] }),
        ),
        ("deleted-artifact-no-members", deleted.clone(), json!({})),
        (
            "never-created-id",
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:artifact/{}", "0".repeat(32)),
            json!({ "ArtifactName": "a-new-name" }),
        ),
        (
            "name-shaped-id",
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:artifact/not-an-id"),
            json!({ "Properties": { "probe": "unknown" } }),
        ),
        (
            "same-id-other-region",
            present.replace("us-east-1", "eu-west-2"),
            json!({ "ArtifactName": "a-new-name" }),
        ),
        (
            "same-id-other-partition",
            present.replace("arn:aws:", "arn:aws-us-gov:"),
            json!({ "ArtifactName": "a-new-name" }),
        ),
    ];

    for (case, arn, update) in &cases {
        let mut body = update
            .as_object()
            .expect("every row's update body is an object")
            .clone();
        body.insert("ArtifactArn".to_string(), json!(arn));
        let (status, parsed) = emulator.call("UpdateArtifact", &Value::Object(body)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an artifact the account does not hold must be refused, \
             got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {case:?}: the refusal must use the operation's modelled not-found error, \
             got {parsed:?}"
        );
        assert_eq!(
            message(&parsed),
            not_found_message(arn),
            "case {case:?}: the refusal sentence, got {parsed:?}"
        );
        // The refused update created nothing: the ARN is still unreadable.
        let (read_status, read) = emulator
            .call("DescribeArtifact", &json!({ "ArtifactArn": arn }))
            .await;
        assert_eq!(
            read_status, 400,
            "case {case:?}: a refused update must not mint an artifact, got {read_status} {read}"
        );
    }

    // The artifact that does exist is untouched, and still updatable.
    let described = emulator
        .ok("DescribeArtifact", &json!({ "ArtifactArn": present }))
        .await;
    assert_eq!(
        described.get("Properties"),
        Some(&json!({ "stage": "raw" })),
        "case present: the live artifact must be unchanged, got {described:?}"
    );
    let updated = emulator
        .ok(
            "UpdateArtifact",
            &json!({ "ArtifactArn": present, "Properties": { "stage": "curated" } }),
        )
        .await;
    assert_eq!(
        updated.get("ArtifactArn").and_then(Value::as_str),
        Some(present.as_str()),
        "case present: the live artifact is still updatable, got {updated:?}"
    );
}

#[tokio::test]
async fn a_refused_update_leaves_the_stored_artifact_exactly_as_it_was() {
    let emulator = Emulator::new();

    let taken = emulator
        .create_artifact("taken-name", Some(json!({ "stage": "first" })))
        .await;
    let subject = emulator
        .create_artifact("subject", Some(json!({ "stage": "raw", "keep": "yes" })))
        .await;

    // Rows differ along the rule the request breaks: the per-Region uniqueness of
    // `ArtifactName`, and the minimum a property VALUE is enforced to carry (the shape
    // `ArtifactProperties` values target, `ArtifactPropertyValue`, models `len 0..4096`,
    // so an empty value passes the modelled bound and is refused by the service).
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "name-already-in-use",
            json!({ "ArtifactName": "taken-name" }),
            taken.as_str(),
        ),
        (
            "name-already-in-use-beside-properties",
            json!({ "ArtifactName": "taken-name", "Properties": { "stage": "curated" } }),
            taken.as_str(),
        ),
        (
            "empty-property-value-alone",
            json!({ "Properties": { "stage": "" } }),
            "",
        ),
        (
            "empty-property-value-beside-filled-entries",
            json!({ "Properties": { "stage": "curated", "empty": "" } }),
            "",
        ),
    ];

    for (case, update, must_name) in &cases {
        let mut body = update
            .as_object()
            .expect("every row's update body is an object")
            .clone();
        body.insert("ArtifactArn".to_string(), json!(subject));
        let (status, parsed) = emulator.call("UpdateArtifact", &Value::Object(body)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the request breaks a constraint and must be refused, \
             got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case:?}: error code, got {parsed:?}"
        );
        if !must_name.is_empty() {
            assert!(
                message(&parsed).contains(must_name),
                "case {case:?}: the refusal must name the artifact holding the name, \
                 got {parsed:?}"
            );
        }

        // Nothing the refused update named was written.
        let described = emulator
            .ok("DescribeArtifact", &json!({ "ArtifactArn": subject }))
            .await;
        assert_eq!(
            described.get("ArtifactName").and_then(Value::as_str),
            Some("subject"),
            "case {case:?}: the stored name must be unchanged, got {described:?}"
        );
        assert_eq!(
            described.get("Properties"),
            Some(&json!({ "stage": "raw", "keep": "yes" })),
            "case {case:?}: the stored properties must be unchanged, got {described:?}"
        );
    }

    // The artifact may keep the name it already has: that is no collision with itself.
    emulator
        .ok(
            "UpdateArtifact",
            &json!({ "ArtifactArn": subject, "ArtifactName": "subject" }),
        )
        .await;
    // The name is unique per Region, so an artifact of ANOTHER Region may take it.
    let elsewhere = emulator
        .create_artifact_in("eu-west-2", "elsewhere", None)
        .await;
    let renamed = emulator
        .ok_in(
            "eu-west-2",
            "UpdateArtifact",
            &json!({ "ArtifactArn": elsewhere, "ArtifactName": "taken-name" }),
        )
        .await;
    assert_eq!(
        renamed.get("ArtifactArn").and_then(Value::as_str),
        Some(elsewhere.as_str()),
        "case other-region: a name taken in one Region is free in another, got {renamed:?}"
    );
    // And the artifact that holds the name in the first Region still does.
    let described = emulator
        .ok("DescribeArtifact", &json!({ "ArtifactArn": taken }))
        .await;
    assert_eq!(
        described.get("ArtifactName").and_then(Value::as_str),
        Some("taken-name"),
        "case other-region: the first Region's holder is untouched, got {described:?}"
    );
}
