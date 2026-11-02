// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `DeleteArtifact`, the operation that releases a SageMaker
//! ML Lineage artifact: `CreateArtifact` -> `DescribeArtifact` -> `DeleteArtifact` ->
//! `DescribeArtifact` refused.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the
//! delete resolves the artifact by `ArtifactArn` when the request carries one and
//! otherwise by the `Source`'s URI within the Region the request signed for; it answers
//! the ARN of the artifact it removed; the removal is total (a later read, and a second
//! delete, are refused with the modelled `ResourceNotFound`) and releases the name; and
//! a request identifying no artifact at all is refused as a bad request.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
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
        self.dispatch(request).await
    }

    /// Call `target` unsigned: no credential scope, so the Region falls back to the
    /// emulator default (`state::DEFAULT_REGION`, `us-east-1`).
    async fn call_unsigned(&self, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.dispatch(request).await
    }

    async fn dispatch(&self, request: http::Request<bytes::Bytes>) -> (u16, Value) {
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

    /// Create an artifact in `region` and return the ARN the server minted.
    async fn create_in(&self, region: &str, body: &Value) -> String {
        let created = self.ok_in(region, "CreateArtifact", body).await;
        str_field(&created, "ArtifactArn")
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

/// A source URI of exactly `len` characters, so a case can sit on a published bound
/// (`SourceUri` is `len 1..2048`).
fn source_uri_of_len(len: usize) -> String {
    let head = "s3://bucket/";
    format!("{head}{}", "a".repeat(len - head.len()))
}

/// `Source` as a reader reports it for an artifact created with `body`: the source the
/// create sent, with `SourceTypes` as the LIST the artifact holds — the empty list when the
/// create named none, because the member reports what the artifact holds rather than what
/// the request mentioned.
fn reported_source(body: &Value) -> Value {
    let mut source = body
        .get("Source")
        .cloned()
        .expect("every create in this table carries the required Source");
    if source.get("SourceTypes").is_none() {
        source["SourceTypes"] = json!([]);
    }
    source
}

/// RULE: an `ArtifactArn` is `arn:aws:sagemaker:<signed region>:<account>:artifact/<id>`,
/// the form the modelled `ArtifactArn` pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:artifact/`) admits.
fn assert_artifact_arn(case: &str, arn: &str, region: &str) {
    let prefix = format!("arn:aws:sagemaker:{region}:{ACCOUNT}:artifact/");
    assert!(
        arn.starts_with(&prefix),
        "case {case:?}: expected an ARN starting {prefix:?}, got {arn:?}"
    );
}

/// RULE: once the artifact is gone, every read of it is refused with the operation's
/// modelled `ResourceNotFound` (HTTP 400) naming the identifier the caller passed.
async fn assert_gone(emulator: &Emulator, case: &str, region: &str, arn: &str) {
    for target in ["DescribeArtifact", "DeleteArtifact"] {
        let (status, parsed) = emulator
            .call_in(region, target, &json!({ "ArtifactArn": arn }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: {target} on a deleted artifact must be refused, \
             got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {case:?}: {target} must use the modelled not-found error, got {parsed:?}"
        );
        assert!(
            message(&parsed).contains(arn),
            "case {case:?}: {target} must name the identifier it could not find, \
             got {parsed:?}"
        );
    }
}

#[tokio::test]
async fn deleting_by_arn_answers_that_arn_and_removes_only_that_artifact() {
    let emulator = Emulator::new();

    // Rows differ along what the rule generalises over: the Region (including the
    // unsigned fallback), whether the artifact is named, the shape and size of the
    // source URI, the artifact type, and how many properties and source types it
    // carries.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "minimal-unnamed",
            "us-east-1",
            json!({
                "Source": { "SourceUri": "s3://bucket/minimal.csv" },
                "ArtifactType": "DataSet",
            }),
        ),
        (
            "named-with-every-member",
            "us-east-1",
            json!({
                "ArtifactName": "every-member",
                "Source": {
                    "SourceUri": "s3://bucket/train/data.csv",
                    "SourceTypes": [
                        { "SourceIdType": "Custom", "Value": "sha256:abc" },
                        { "SourceIdType": "MD5Hash", "Value": "d41d8cd98f00b204e9800998ecf8427e" },
                    ],
                },
                "ArtifactType": "DataSet",
                "Properties": { "stage": "train", "rows": "1024" },
                "MetadataProperties": { "CommitId": "0f1e2d3c" },
                "Tags": [{ "Key": "team", "Value": "ml" }],
            }),
        ),
        (
            "empty-artifact-type-in-another-region",
            "eu-west-2",
            json!({
                // `String256` is `len 0..256`, so the empty string is a legal type.
                "ArtifactName": "empty-type",
                "Source": { "SourceUri": "s3://bucket/empty-type.csv" },
                "ArtifactType": "",
            }),
        ),
        (
            "unicode-source-uri",
            "ap-southeast-1",
            json!({
                "Source": { "SourceUri": "s3://bucket/café/数据/😀.csv" },
                "ArtifactType": "Model",
                "Properties": { "libellé": "café" },
            }),
        ),
        (
            "max-source-uri-in-a-gov-region",
            "us-gov-west-1",
            json!({
                "Source": { "SourceUri": source_uri_of_len(2048) },
                "ArtifactType": "DataSet",
            }),
        ),
        (
            "single-character-source-uri",
            "us-east-1",
            json!({
                "ArtifactName": "min-source-uri",
                "Source": { "SourceUri": "s" },
                "ArtifactType": "DataSet",
            }),
        ),
    ];

    // A bystander artifact per Region: the delete must leave every other record alone.
    let mut bystanders: Vec<(&str, String)> = Vec::new();
    for (case, region, _) in &cases {
        let arn = emulator
            .create_in(
                region,
                &json!({
                    "Source": { "SourceUri": format!("s3://bucket/bystander-{case}.csv") },
                    "ArtifactType": "DataSet",
                }),
            )
            .await;
        bystanders.push((region, arn));
    }

    for (case, region, body) in &cases {
        let arn = emulator.create_in(region, body).await;
        assert_artifact_arn(case, &arn, region);
        // The round trip the delete is about to undo.
        let described = emulator
            .ok_in(region, "DescribeArtifact", &json!({ "ArtifactArn": arn }))
            .await;
        assert_eq!(
            described.get("Source"),
            Some(&reported_source(body)),
            "case {case:?}: the created artifact must be readable before the delete, \
             got {described:?}"
        );

        let deleted = emulator
            .ok_in(region, "DeleteArtifact", &json!({ "ArtifactArn": arn }))
            .await;
        assert_eq!(
            deleted.get("ArtifactArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: DeleteArtifact must answer the ARN of the artifact it \
             removed, got {deleted:?}"
        );

        assert_gone(&emulator, case, region, &arn).await;
    }

    // Every bystander survived every delete.
    for (region, arn) in &bystanders {
        let described = emulator
            .ok_in(region, "DescribeArtifact", &json!({ "ArtifactArn": arn }))
            .await;
        assert_eq!(
            described.get("ArtifactArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case bystander-{region}: an unrelated artifact must survive, got {described:?}"
        );
    }

    // An unsigned delete addresses the emulator's default Region, the one an unsigned
    // create minted into.
    let (status, created) = emulator
        .call_unsigned(
            "CreateArtifact",
            &json!({
                "Source": { "SourceUri": "s3://bucket/unsigned.csv" },
                "ArtifactType": "DataSet",
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case unsigned-create: expected HTTP 200, got {status} with {created}"
    );
    let unsigned_arn = str_field(&created, "ArtifactArn");
    assert_artifact_arn("unsigned-create", &unsigned_arn, "us-east-1");
    let (status, deleted) = emulator
        .call_unsigned("DeleteArtifact", &json!({ "ArtifactArn": unsigned_arn }))
        .await;
    assert_eq!(
        status, 200,
        "case unsigned-delete: expected HTTP 200, got {status} with {deleted}"
    );
    assert_eq!(
        deleted.get("ArtifactArn").and_then(Value::as_str),
        Some(unsigned_arn.as_str()),
        "case unsigned-delete: the answered ARN is the deleted artifact's, got {deleted:?}"
    );
}

#[tokio::test]
async fn deleting_by_source_resolves_the_artifact_with_that_source_uri() {
    let emulator = Emulator::new();

    // Rows differ along the shape and size of the source URI, and along what the DELETE
    // request carries beside it: the URI alone, or the URI with source types that match
    // the create's, or with source types that do not — the URI is what identifies a
    // source (it is also the only source member `ListArtifacts` filters on), so the
    // source types never change which artifact is resolved.
    let cases: Vec<(&str, String, Value)> = vec![
        (
            "uri-alone",
            "s3://bucket/plain.csv".to_string(),
            Value::Null,
        ),
        (
            "unicode-uri-with-matching-source-types",
            "s3://bucket/café/数据/😀.csv".to_string(),
            json!([{ "SourceIdType": "Custom", "Value": "sha256:abc" }]),
        ),
        (
            "percent-encoded-uri-with-other-source-types",
            "s3://bucket/a%20b/c+d?q=1&r=2#frag".to_string(),
            json!([{ "SourceIdType": "MD5Hash", "Value": "d41d8cd98f00b204e9800998ecf8427e" }]),
        ),
        (
            "max-length-uri",
            source_uri_of_len(2048),
            json!([{ "SourceIdType": "S3ETag", "Value": "etag" }]),
        ),
        ("single-character-uri", "s".to_string(), Value::Null),
        (
            "control-byte-uri",
            "s3://bucket/\u{0}\u{1}\u{7f}/raw.bin".to_string(),
            json!([]),
        ),
    ];

    for (index, (case, uri, delete_source_types)) in cases.iter().enumerate() {
        let mut create_source = json!({ "SourceUri": uri });
        if !delete_source_types.is_null() {
            create_source["SourceTypes"] =
                json!([{ "SourceIdType": "Custom", "Value": "sha256:abc" }]);
        }
        let arn = emulator
            .create_in(
                "us-east-1",
                &json!({
                    "ArtifactName": format!("by-source-{index}"),
                    "Source": create_source,
                    "ArtifactType": "DataSet",
                }),
            )
            .await;

        let mut delete_source = json!({ "SourceUri": uri });
        if !delete_source_types.is_null() {
            delete_source["SourceTypes"] = delete_source_types.clone();
        }
        let deleted = emulator
            .ok("DeleteArtifact", &json!({ "Source": delete_source }))
            .await;
        assert_eq!(
            deleted.get("ArtifactArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: a source-addressed delete must answer the ARN of the \
             artifact holding that source URI, got {deleted:?}"
        );

        assert_gone(&emulator, case, "us-east-1", &arn).await;

        // The source is free again: the same URI can be recorded by a later create, and
        // deleting by source once more resolves that NEW artifact.
        let again = emulator
            .create_in(
                "us-east-1",
                &json!({
                    "Source": { "SourceUri": uri },
                    "ArtifactType": "Model",
                }),
            )
            .await;
        assert_ne!(
            again, arn,
            "case {case:?}: the second create must mint its own ARN"
        );
        let deleted_again = emulator
            .ok("DeleteArtifact", &json!({ "Source": { "SourceUri": uri } }))
            .await;
        assert_eq!(
            deleted_again.get("ArtifactArn").and_then(Value::as_str),
            Some(again.as_str()),
            "case {case:?}: the second source-addressed delete must resolve the \
             re-created artifact, got {deleted_again:?}"
        );
    }
}

#[tokio::test]
async fn a_source_addressed_delete_only_reaches_the_region_the_request_signed_for() {
    let emulator = Emulator::new();

    // The same source URI recorded in several Regions: each Region's artifact is its
    // own record, so a delete signed for one Region never removes another's.
    let uri = "s3://bucket/shared-across-regions.csv";
    let regions = ["us-east-1", "eu-west-2", "ap-southeast-1", "us-gov-west-1"];
    let mut arns: Vec<(&str, String)> = Vec::new();
    for region in regions {
        let arn = emulator
            .create_in(
                region,
                &json!({
                    "ArtifactName": "shared-uri",
                    "Source": { "SourceUri": uri },
                    "ArtifactType": "DataSet",
                }),
            )
            .await;
        assert_artifact_arn(region, &arn, region);
        arns.push((region, arn));
    }

    for (index, (region, arn)) in arns.iter().enumerate() {
        let deleted = emulator
            .ok_in(
                region,
                "DeleteArtifact",
                &json!({ "Source": { "SourceUri": uri } }),
            )
            .await;
        assert_eq!(
            deleted.get("ArtifactArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {region}: the delete must resolve the artifact of the Region it was \
             signed for, got {deleted:?}"
        );
        assert_gone(&emulator, region, region, arn).await;

        // Every Region not yet deleted from still holds its artifact.
        for (other_region, other_arn) in arns.iter().skip(index + 1) {
            let described = emulator
                .ok_in(
                    other_region,
                    "DescribeArtifact",
                    &json!({ "ArtifactArn": other_arn }),
                )
                .await;
            assert_eq!(
                described.get("ArtifactArn").and_then(Value::as_str),
                Some(other_arn.as_str()),
                "case {region}->{other_region}: another Region's artifact must survive, \
                 got {described:?}"
            );
        }
    }
}

#[tokio::test]
async fn several_artifacts_sharing_a_source_uri_are_deleted_oldest_first() {
    let emulator = Emulator::new();

    // Nothing stops two creates recording the same source URI, so a source-addressed
    // delete resolves the OLDEST match and successive deletes walk the group in
    // creation order. The sleep is what makes the creation instants distinct rather
    // than a coincidence of clock resolution.
    let uri = "s3://bucket/duplicated.csv";
    let mut created: Vec<String> = Vec::new();
    for index in 0..4 {
        created.push(
            emulator
                .create_in(
                    "us-east-1",
                    &json!({
                        "ArtifactName": format!("duplicate-{index}"),
                        "Source": { "SourceUri": uri },
                        "ArtifactType": "DataSet",
                    }),
                )
                .await,
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(
        created.iter().collect::<BTreeSet<_>>().len(),
        created.len(),
        "case setup: each create must mint its own ARN, got {created:?}"
    );

    for (index, expected) in created.iter().enumerate() {
        let deleted = emulator
            .ok("DeleteArtifact", &json!({ "Source": { "SourceUri": uri } }))
            .await;
        assert_eq!(
            deleted.get("ArtifactArn").and_then(Value::as_str),
            Some(expected.as_str()),
            "case duplicate-{index}: the delete must take the oldest artifact still \
             holding the source URI, got {deleted:?}"
        );
        assert_gone(&emulator, &format!("duplicate-{index}"), "us-east-1", expected).await;
    }

    // The group is empty, so the next source-addressed delete finds nothing.
    let (status, parsed) = emulator
        .call("DeleteArtifact", &json!({ "Source": { "SourceUri": uri } }))
        .await;
    assert_eq!(
        status, 400,
        "case exhausted: a source nothing holds must be refused, got {status} {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ResourceNotFound",
        "case exhausted: error code, got {parsed:?}"
    );
}

#[tokio::test]
async fn an_arn_in_the_request_resolves_the_delete_even_beside_a_source() {
    let emulator = Emulator::new();

    // Rows: the source sent beside the ARN is the ARN artifact's own, another stored
    // artifact's, or one nothing recorded. The ARN identifies the artifact directly, so
    // every row removes exactly the ARN's artifact.
    let other_uri = "s3://bucket/other.csv";
    let other_arn = emulator
        .create_in(
            "us-east-1",
            &json!({
                "ArtifactName": "other",
                "Source": { "SourceUri": other_uri },
                "ArtifactType": "DataSet",
            }),
        )
        .await;

    let cases: Vec<(&str, Value)> = vec![
        // Filled in per row below: the artifact's own source URI carries the row index.
        ("its-own-source", Value::Null),
        ("another-artifacts-source", json!({ "SourceUri": other_uri })),
        (
            "a-source-nothing-recorded",
            json!({ "SourceUri": "s3://bucket/never-created.csv" }),
        ),
    ];

    for (index, (case, source)) in cases.iter().enumerate() {
        let own_uri = format!("s3://bucket/own-{index}.csv");
        let arn = emulator
            .create_in(
                "us-east-1",
                &json!({
                    "ArtifactName": format!("target-{index}"),
                    "Source": { "SourceUri": own_uri },
                    "ArtifactType": "DataSet",
                }),
            )
            .await;
        let source = if *case == "its-own-source" {
            json!({ "SourceUri": own_uri })
        } else {
            source.clone()
        };

        let deleted = emulator
            .ok(
                "DeleteArtifact",
                &json!({ "ArtifactArn": arn, "Source": source }),
            )
            .await;
        assert_eq!(
            deleted.get("ArtifactArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: the ARN in the request must be what the delete resolves, \
             got {deleted:?}"
        );
        assert_gone(&emulator, case, "us-east-1", &arn).await;

        // The artifact whose source was quoted beside the ARN is untouched.
        let described = emulator
            .ok("DescribeArtifact", &json!({ "ArtifactArn": other_arn }))
            .await;
        assert_eq!(
            described.get("ArtifactArn").and_then(Value::as_str),
            Some(other_arn.as_str()),
            "case {case:?}: the artifact named only by source must survive, \
             got {described:?}"
        );
    }
}

#[tokio::test]
async fn a_delete_that_identifies_no_artifact_is_refused_as_a_bad_request() {
    let emulator = Emulator::new();

    // The model documents "Either ArtifactArn or Source must be specified", and both
    // members are optional on the wire: a request carrying neither is a bad request,
    // never a missing artifact.
    let (status, parsed) = emulator.call("DeleteArtifact", &json!({})).await;
    assert_eq!(
        status, 400,
        "case no-identifier: a delete naming no artifact must be refused, \
         got {status} {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ValidationException",
        "case no-identifier: error code, got {parsed:?}"
    );

    // And nothing was removed by that refusal.
    let arn = emulator
        .create_in(
            "us-east-1",
            &json!({
                "Source": { "SourceUri": "s3://bucket/survivor.csv" },
                "ArtifactType": "DataSet",
            }),
        )
        .await;
    let (status, parsed) = emulator.call("DeleteArtifact", &json!({})).await;
    assert_eq!(
        status, 400,
        "case no-identifier-with-a-stored-artifact: expected a refusal, \
         got {status} {parsed}"
    );
    let described = emulator
        .ok("DescribeArtifact", &json!({ "ArtifactArn": arn }))
        .await;
    assert_eq!(
        described.get("ArtifactArn").and_then(Value::as_str),
        Some(arn.as_str()),
        "case no-identifier-with-a-stored-artifact: the stored artifact must survive, \
         got {described:?}"
    );
}

#[tokio::test]
async fn deleting_an_identifier_nothing_created_is_refused_with_resource_not_found() {
    let emulator = Emulator::new();

    let present = emulator
        .create_in(
            "us-east-1",
            &json!({
                "ArtifactName": "present",
                "Source": { "SourceUri": "s3://bucket/present.csv" },
                "ArtifactType": "DataSet",
            }),
        )
        .await;

    // Rows: identifiers inside the modelled constraints that name no stored artifact —
    // an ARN of the minted shape, a name-shaped id, the stored artifact's id in another
    // Region and in another partition, and source URIs nothing recorded (including the
    // published bounds of `SourceUri`, len 1..2048).
    let cases: Vec<(&str, Value)> = vec![
        (
            "unknown-hex-id",
            json!({
                "ArtifactArn":
                    format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:artifact/{}", "0".repeat(32)),
            }),
        ),
        (
            "name-shaped-id",
            json!({ "ArtifactArn": format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:artifact/not-an-id") }),
        ),
        (
            "same-id-other-region",
            json!({ "ArtifactArn": present.replace("us-east-1", "eu-west-2") }),
        ),
        (
            "same-id-other-partition",
            json!({ "ArtifactArn": present.replace("arn:aws:", "arn:aws-us-gov:") }),
        ),
        (
            "unknown-source-uri",
            json!({ "Source": { "SourceUri": "s3://bucket/never-created.csv" } }),
        ),
        (
            "unknown-single-character-source-uri",
            json!({ "Source": { "SourceUri": "x" } }),
        ),
        (
            "unknown-max-length-source-uri",
            json!({ "Source": { "SourceUri": source_uri_of_len(2048) } }),
        ),
        (
            "a-prefix-of-a-stored-source-uri",
            json!({ "Source": { "SourceUri": "s3://bucket/present" } }),
        ),
        (
            "the-stored-source-uri-in-another-region",
            json!({ "Source": { "SourceUri": "s3://bucket/present.csv" } }),
        ),
    ];

    for (case, body) in &cases {
        // The last row is signed for a Region that holds nothing; every other row is
        // signed for the Region the stored artifact lives in.
        let region = if *case == "the-stored-source-uri-in-another-region" {
            "eu-west-2"
        } else {
            "us-east-1"
        };
        let (status, parsed) = emulator.call_in(region, "DeleteArtifact", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an artifact nothing created must be refused, \
             got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {case:?}: the refusal must use the operation's modelled not-found \
             error, got {parsed:?}"
        );
    }

    // The artifact that does exist was never touched by those refusals, and deleting it
    // frees its name for a later create.
    let described = emulator
        .ok("DescribeArtifact", &json!({ "ArtifactArn": present }))
        .await;
    assert_eq!(
        described.get("ArtifactName").and_then(Value::as_str),
        Some("present"),
        "case present: the stored artifact must still be readable, got {described:?}"
    );
    emulator
        .ok("DeleteArtifact", &json!({ "ArtifactArn": present }))
        .await;
    let reborn = emulator
        .create_in(
            "us-east-1",
            &json!({
                "ArtifactName": "present",
                "Source": { "SourceUri": "s3://bucket/present-again.csv" },
                "ArtifactType": "DataSet",
            }),
        )
        .await;
    assert_ne!(
        reborn, present,
        "case name-freed: the re-created artifact must carry its own ARN"
    );
}
