// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests pinning that `UpdateArtifact` is a STATE WRITE, not a reply:
//! what it applies is observable through every implemented reader of that artifact, and
//! through no other artifact.
//!
//! `tests/all/artifact_updates.rs` already pins the members an update applies as
//! `DescribeArtifact` replays them. This suite pins the same rule against the OTHER
//! readers of a stored artifact — the `ListArtifacts` summary and the lineage end a
//! `ListAssociations` summary resolves — so an update that answered 200 while storing
//! nothing, or that stored only where one reader looks, fails here.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

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

    async fn call(&self, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/sagemaker/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0000",
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

    /// Call `target` and require a 200, returning the parsed output.
    async fn ok(&self, case: &str, target: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(target, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {target}, got {status} with {parsed}"
        );
        parsed
    }

    /// Create one artifact and answer the `ArtifactArn` it was minted under.
    async fn create_artifact(&self, case: &str, name: &str, properties: Option<Value>) -> String {
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
            .ok(case, "CreateArtifact", &Value::Object(body))
            .await;
        str_field(&created, "ArtifactArn")
    }

    /// The `ListArtifacts` summary for `arn`, from a page wide enough to hold the whole
    /// (two-artifact) store.
    async fn summary_of(&self, case: &str, arn: &str) -> Value {
        let listed = self
            .ok(case, "ListArtifacts", &json!({ "MaxResults": 50 }))
            .await;
        listed
            .get("ArtifactSummaries")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected ArtifactSummaries in {listed}"))
            .iter()
            .find(|summary| summary.get("ArtifactArn").and_then(Value::as_str) == Some(arn))
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a summary for {arn} in {listed}")
            })
            .clone()
    }

    /// The single `ListAssociations` summary of the edge whose source is `source_arn`.
    async fn association_from(&self, case: &str, source_arn: &str) -> Value {
        let listed = self
            .ok(case, "ListAssociations", &json!({ "SourceArn": source_arn }))
            .await;
        let summaries = listed
            .get("AssociationSummaries")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected AssociationSummaries in {listed}"));
        assert_eq!(
            summaries.len(),
            1,
            "case {case:?}: the one stored edge must be reported, got {listed}"
        );
        summaries[0].clone()
    }
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
/// 120 and on the published minimum of 1
/// (`smithy_query(query="operation",operation="UpdateArtifact")`: `ArtifactName` len
/// 1..120).
fn name_of_len(len: usize) -> String {
    "n".repeat(len)
}

/// A `Properties` map of `count` entries, for a case that differs along the ENTRY COUNT.
/// 30 is the modelled maximum (`ArtifactProperties`, len 0..30).
fn properties_of(count: usize) -> Value {
    let map: Map<String, Value> = (0..count)
        .map(|index| (format!("key{index}"), json!(format!("value{index}"))))
        .collect();
    Value::Object(map)
}

/// One row: what the subject artifact was created with, the update sent to it, and what
/// EVERY reader of that artifact must report afterwards.
struct Row {
    case: &'static str,
    created_properties: Option<Value>,
    /// The update body, without the `ArtifactArn` the test fills in.
    update: Value,
    /// The name the artifact answers to afterwards, whichever reader is asked.
    expected_name: String,
    /// `Properties` as `DescribeArtifact` must report it — the map the artifact HOLDS.
    expected_properties: Value,
}

#[tokio::test]
async fn an_update_is_visible_through_every_reader_of_the_artifact() {
    let long_name = name_of_len(120);
    let shortest_name = name_of_len(1);
    // Rows differ along what the rule generalises over: which members the update carries,
    // the length of the new name (the published minimum and maximum among them), and the
    // size and shape of the property map (absent, emptied, replaced, pruned, and the
    // modelled maximum entry count).
    let rows = vec![
        Row {
            case: "rename-only",
            created_properties: Some(json!({ "stage": "raw" })),
            update: json!({ "ArtifactName": "renamed-subject" }),
            expected_name: "renamed-subject".to_string(),
            expected_properties: json!({ "stage": "raw" }),
        },
        Row {
            case: "rename-to-the-published-minimum-length",
            created_properties: None,
            update: json!({ "ArtifactName": shortest_name.clone() }),
            expected_name: shortest_name.clone(),
            expected_properties: json!({}),
        },
        Row {
            case: "rename-to-the-published-maximum-length",
            created_properties: None,
            update: json!({ "ArtifactName": long_name.clone() }),
            expected_name: long_name.clone(),
            expected_properties: json!({}),
        },
        Row {
            case: "rename-beside-replaced-properties",
            created_properties: Some(json!({ "stage": "raw", "keep": "yes" })),
            update: json!({ "ArtifactName": "renamed-and-repropertied", "Properties": { "stage": "curated" } }),
            expected_name: "renamed-and-repropertied".to_string(),
            expected_properties: json!({ "stage": "curated" }),
        },
        Row {
            case: "rename-beside-the-modelled-maximum-property-count",
            created_properties: Some(json!({ "stage": "raw" })),
            update: json!({ "ArtifactName": "renamed-with-thirty", "Properties": properties_of(30) }),
            expected_name: "renamed-with-thirty".to_string(),
            expected_properties: properties_of(30),
        },
        Row {
            case: "rename-beside-pruned-properties",
            created_properties: Some(json!({ "a": "1", "b": "2" })),
            update: json!({ "ArtifactName": "renamed-and-pruned", "PropertiesToRemove": ["b"] }),
            expected_name: "renamed-and-pruned".to_string(),
            expected_properties: json!({ "a": "1" }),
        },
        Row {
            case: "properties-only-leaves-the-name",
            created_properties: Some(json!({ "a": "1" })),
            update: json!({ "Properties": { "b": "2" } }),
            expected_name: "subject".to_string(),
            expected_properties: json!({ "b": "2" }),
        },
        Row {
            case: "no-member-beyond-the-arn",
            created_properties: Some(json!({ "a": "1" })),
            update: json!({}),
            expected_name: "subject".to_string(),
            expected_properties: json!({ "a": "1" }),
        },
    ];

    for row in &rows {
        let case = row.case;
        // A fresh store per row, holding exactly the subject and one bystander, so the
        // listing below is one page and the bystander's reading is unambiguous.
        let emulator = Emulator::new();
        let subject = emulator
            .create_artifact(case, "subject", row.created_properties.clone())
            .await;
        let bystander = emulator.create_artifact(case, "bystander", None).await;
        // One lineage edge out of the subject, so the update is also read back through the
        // entity-name resolution `ListAssociations` performs.
        emulator
            .ok(
                case,
                "AddAssociation",
                &json!({
                    "SourceArn": subject,
                    "DestinationArn": bystander,
                    "AssociationType": "Produced",
                }),
            )
            .await;

        let subject_before = emulator.summary_of(case, &subject).await;
        let bystander_before = emulator.summary_of(case, &bystander).await;
        let modified_before = f64_field(case, &subject_before, "LastModifiedTime");

        let mut update = row
            .update
            .as_object()
            .expect("every row's update body is an object")
            .clone();
        update.insert("ArtifactArn".to_string(), json!(subject));
        emulator
            .ok(case, "UpdateArtifact", &Value::Object(update))
            .await;

        // Reader 1: the artifact's own Describe.
        let described = emulator
            .ok(case, "DescribeArtifact", &json!({ "ArtifactArn": subject }))
            .await;
        assert_eq!(
            described.get("ArtifactName").and_then(Value::as_str),
            Some(row.expected_name.as_str()),
            "case {case:?}: DescribeArtifact must report the updated name, got {described:?}"
        );
        assert_eq!(
            described.get("Properties"),
            Some(&row.expected_properties),
            "case {case:?}: DescribeArtifact must report the updated properties, \
             got {described:?}"
        );

        // Reader 2: the listing's summary of the same artifact.
        let subject_after = emulator.summary_of(case, &subject).await;
        assert_eq!(
            subject_after.get("ArtifactName").and_then(Value::as_str),
            Some(row.expected_name.as_str()),
            "case {case:?}: the ListArtifacts summary must report the updated name, \
             got {subject_after:?}"
        );
        let modified_after = f64_field(case, &subject_after, "LastModifiedTime");
        assert!(
            modified_after >= modified_before,
            "case {case:?}: the summary's LastModifiedTime must not go backwards, \
             got {modified_after} after {modified_before}"
        );
        assert_eq!(
            subject_after.get("CreationTime"),
            subject_before.get("CreationTime"),
            "case {case:?}: an update never moves CreationTime, got {subject_after:?}"
        );
        assert_eq!(
            subject_after.get("Source"),
            subject_before.get("Source"),
            "case {case:?}: an update never touches a member it does not carry, \
             got {subject_after:?}"
        );
        assert_eq!(
            subject_after.get("ArtifactType"),
            subject_before.get("ArtifactType"),
            "case {case:?}: an update never touches a member it does not carry, \
             got {subject_after:?}"
        );

        // Reader 3: the lineage end a ListAssociations summary resolves out of the store.
        let edge = emulator.association_from(case, &subject).await;
        assert_eq!(
            edge.get("SourceName").and_then(Value::as_str),
            Some(row.expected_name.as_str()),
            "case {case:?}: the association's resolved source name must be the updated \
             one, got {edge:?}"
        );
        assert_eq!(
            edge.get("DestinationName").and_then(Value::as_str),
            Some("bystander"),
            "case {case:?}: updating one artifact never renames another, got {edge:?}"
        );

        // No other artifact moved: the bystander's whole summary is byte-for-byte the one
        // read before the update.
        assert_eq!(
            emulator.summary_of(case, &bystander).await,
            bystander_before,
            "case {case:?}: updating one artifact must leave every other artifact exactly \
             as it was"
        );
        let bystander_described = emulator
            .ok(case, "DescribeArtifact", &json!({ "ArtifactArn": bystander }))
            .await;
        assert_eq!(
            bystander_described.get("Properties"),
            Some(&json!({})),
            "case {case:?}: the bystander's properties must be untouched, \
             got {bystander_described:?}"
        );
    }
}
