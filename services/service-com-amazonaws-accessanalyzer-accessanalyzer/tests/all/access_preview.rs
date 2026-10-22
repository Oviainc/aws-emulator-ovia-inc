// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the access-preview slice, driven over the wire.
//!
//! Every case here goes through the real router: an `http::Request` is dispatched to
//! the mounted service, so the restJson1 routing, the boundary constraint validation,
//! the handler and the response serialization all run exactly as they do in the
//! emulator binary. Nothing reaches into `crate::state` directly — a created preview is
//! only believed when a later `GetAccessPreview` reports it back.
//!
//! The tables span the classes the rules generalise over (every `Configuration` union
//! member that carries a policy document and two that carry none, every analyzer
//! `Type`, the forms a proposed configuration can be unanalyzable in, the size of the
//! `configurations` map); the recorded public-S3-bucket-policy and IAM-role-trust-policy
//! cases from the issue are two rows.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_accessanalyzer_accessanalyzer::state::{
    ACCESS_PREVIEW_ANALYSIS_SPAN, MAX_ACCESS_PREVIEWS_PER_ANALYZER,
};
use service_com_amazonaws_accessanalyzer_accessanalyzer::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for; it is the region every analyzer ARN carries.
const REGION: &str = "us-east-1";

/// How long a case waits past [`ACCESS_PREVIEW_ANALYSIS_SPAN`] before reading the
/// terminal status, so a loaded machine cannot read the preview a moment early.
const SETTLE_MARGIN: Duration = Duration::from_millis(250);

/// One response, reduced to what the assertions read.
#[derive(Debug)]
struct Reply {
    status: u16,
    /// The wire error code: restJson1 resolves an error shape from
    /// `x-amzn-errortype`, falling back to the body's `__type`.
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    fn code(&self) -> &str {
        self.error_code.as_deref().unwrap_or("")
    }

    /// A member of the response body, treating JSON `null` as absent the way an
    /// omitted optional member is.
    fn member(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|value| !value.is_null())
    }

    /// The `accessPreview` of a `GetAccessPreview` reply.
    fn preview(&self) -> &Value {
        self.member("accessPreview")
            .unwrap_or_else(|| panic!("expected an accessPreview in {:?}", self.body))
    }

    /// The `id` a `CreateAccessPreview` reply carries.
    fn preview_id(&self) -> String {
        self.member("id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("expected an id in {:?}", self.body))
            .to_string()
    }
}

/// The emulator under test: one engine, one mounted service, one router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match body {
            Some(value) => serde_json::to_vec(&value).expect("the request body serializes"),
            None => Vec::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://access-analyzer.{REGION}.amazonaws.com{path}"))
            // The SigV4 credential scope is what routes the request to this service
            // and what names the region its ARNs carry. Signatures are not verified
            // unless the process turns authentication on.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/access-analyzer/aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        let error_code = error_code.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        Reply {
            status,
            error_code,
            body,
        }
    }

    /// Create an analyzer and answer the ARN it reports, which is what every
    /// preview call is scoped by.
    async fn analyzer_arn(&self, name: &str, r#type: &str) -> String {
        let reply = self
            .send(
                "PUT",
                "/analyzer",
                Some(json!({"analyzerName": name, "type": r#type})),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {name}/{type}: CreateAnalyzer must succeed, got {reply:?}"
        );
        reply
            .member("arn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {name}: expected an arn in {:?}", reply.body))
            .to_string()
    }

    async fn create_access_preview(&self, body: Value) -> Reply {
        self.send("PUT", "/access-preview", Some(body)).await
    }

    async fn get_access_preview(&self, id: &str, analyzer_arn: &str) -> Reply {
        self.send(
            "GET",
            &format!("/access-preview/{id}?analyzerArn={analyzer_arn}"),
            None,
        )
        .await
    }

    async fn delete_analyzer(&self, name: &str) -> Reply {
        self.send("DELETE", &format!("/analyzer/{name}"), None).await
    }
}

/// Does `candidate` satisfy the model's `AccessPreviewId` pattern
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`? Checked against
/// the rendered form rather than through the minting code, so an id that is not in
/// that form cannot make a case pass.
fn is_access_preview_id(candidate: &str) -> bool {
    let groups: Vec<&str> = candidate.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12]) // the 8-4-4-4-12 layout
        && groups
            .iter()
            .all(|group| group.chars().all(|digit| digit.is_ascii_hexdigit() && !digit.is_uppercase()))
}

/// `value` with every `null` member dropped, recursively.
///
/// A member the request omitted comes back as an explicit `null` in restJson1, so this
/// is what lets a round-trip case compare the configuration it sent against the
/// configuration it got back.
fn without_nulls(value: &Value) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(_, member)| !member.is_null())
                .map(|(name, member)| (name.clone(), without_nulls(member)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_nulls).collect()),
        other => other.clone(),
    }
}

/// A policy document that grants access to everyone — the recorded public bucket
/// policy's shape.
fn public_policy(resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": "*",
            "Action": "s3:GetObject",
            "Resource": format!("{resource}/*"),
        }],
    })
    .to_string()
}

/// A policy document that grants access to one account only.
fn scoped_policy(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": principal},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// Every `Configuration` union member the service accepts, from
/// `smithy_query(query="shape", shape="Configuration")`: eleven that carry a policy
/// document and two (`ebsSnapshot`, `rdsDbSnapshot`) that share through attributes
/// instead. Each row is `(case, resource ARN, configuration)`.
fn analyzable_configurations() -> Vec<(&'static str, String, Value)> {
    let bucket = "arn:aws:s3:::preview-bucket".to_string();
    vec![
        (
            // The recorded `access-preview-detects-public-s3-bucket-policy` case.
            "s3 bucket with a public bucket policy",
            bucket.clone(),
            json!({"s3Bucket": {"bucketPolicy": public_policy(&bucket)}}),
        ),
        (
            // The recorded `access-preview-private-bucket-policy-no-external-finding` case.
            "s3 bucket with a scoped bucket policy, an acl grant and a public access block",
            bucket.clone(),
            json!({"s3Bucket": {
                "bucketPolicy": scoped_policy("000000000000"),
                "bucketAclGrants": [{"permission": "READ", "grantee": {"id": "abc123"}}],
                "bucketPublicAccessBlock": {
                    "ignorePublicAcls": true,
                    "restrictPublicBuckets": true,
                },
            }}),
        ),
        (
            "s3 bucket with an access point policy",
            bucket.clone(),
            json!({"s3Bucket": {
                "accessPoints": {
                    "arn:aws:s3:us-east-1:000000000000:accesspoint/preview-point": {
                        "accessPointPolicy": scoped_policy("000000000000"),
                        "networkOrigin": {"internetConfiguration": {}},
                    },
                },
            }}),
        ),
        (
            // The recorded `access-preview-iam-role-external-trust-policy-finding` case.
            "iam role with an external trust policy",
            "arn:aws:iam::000000000000:role/preview-role".to_string(),
            json!({"iamRole": {"trustPolicy": scoped_policy("111111111111")}}),
        ),
        (
            "sqs queue policy",
            "arn:aws:sqs:us-east-1:000000000000:preview-queue".to_string(),
            json!({"sqsQueue": {"queuePolicy": scoped_policy("111111111111")}}),
        ),
        (
            "sns topic policy",
            "arn:aws:sns:us-east-1:000000000000:preview-topic".to_string(),
            json!({"snsTopic": {"topicPolicy": scoped_policy("111111111111")}}),
        ),
        (
            "kms key with two key policies and a grant",
            "arn:aws:kms:us-east-1:000000000000:key/abcd".to_string(),
            json!({"kmsKey": {
                "keyPolicies": {
                    "default": scoped_policy("000000000000"),
                    "other": scoped_policy("111111111111"),
                },
                "grants": [{
                    "operations": ["Decrypt"],
                    "granteePrincipal": "arn:aws:iam::111111111111:role/grantee",
                    "issuingAccount": "000000000000",
                }],
            }}),
        ),
        (
            "secrets manager secret policy",
            "arn:aws:secretsmanager:us-east-1:000000000000:secret:preview".to_string(),
            json!({"secretsManagerSecret": {"secretPolicy": scoped_policy("111111111111")}}),
        ),
        (
            "efs file system policy",
            "arn:aws:elasticfilesystem:us-east-1:000000000000:file-system/fs-1".to_string(),
            json!({"efsFileSystem": {"fileSystemPolicy": scoped_policy("111111111111")}}),
        ),
        (
            "ecr repository policy",
            "arn:aws:ecr:us-east-1:000000000000:repository/preview".to_string(),
            json!({"ecrRepository": {"repositoryPolicy": scoped_policy("111111111111")}}),
        ),
        (
            "dynamodb table policy",
            "arn:aws:dynamodb:us-east-1:000000000000:table/preview".to_string(),
            json!({"dynamodbTable": {"tablePolicy": scoped_policy("111111111111")}}),
        ),
        (
            "dynamodb stream policy",
            "arn:aws:dynamodb:us-east-1:000000000000:table/preview/stream/2026".to_string(),
            json!({"dynamodbStream": {"streamPolicy": scoped_policy("111111111111")}}),
        ),
        (
            "ebs snapshot shared without a policy document",
            "arn:aws:ec2:us-east-1:000000000000:snapshot/snap-1".to_string(),
            json!({"ebsSnapshot": {"userIds": ["111111111111"], "groups": ["all"]}}),
        ),
        (
            "rds snapshot shared without a policy document",
            "arn:aws:rds:us-east-1:000000000000:snapshot:preview".to_string(),
            json!({"rdsDbSnapshot": {
                "attributes": {"restore": {"accountIds": ["111111111111"]}},
            }}),
        ),
    ]
}

/// RULE: `CreateAccessPreview` stores the proposed configuration under a fresh
/// `accessPreviewId` and `GetAccessPreview` rebuilds it — the id, the analyzer ARN, the
/// configurations and a `createdAt` — for every kind of resource configuration the
/// service accepts. The preview is reported `CREATING` until its analysis is due and
/// `COMPLETED` after it.
#[tokio::test]
async fn an_access_preview_reads_back_as_it_was_created() {
    let emulator = Emulator::new();
    let analyzer = emulator.analyzer_arn("preview_analyzer", "ACCOUNT").await;

    let mut created: Vec<(&str, String, Value)> = Vec::new();
    let mut ids: BTreeSet<String> = BTreeSet::new();
    for (case, resource, configuration) in analyzable_configurations() {
        let configurations = json!({resource.clone(): configuration});
        let create = emulator
            .create_access_preview(json!({
                "analyzerArn": analyzer,
                "configurations": configurations,
            }))
            .await;
        assert_eq!(
            create.status, 200,
            "case {case:?}: expected CreateAccessPreview to succeed, got {create:?}"
        );
        let id = create.preview_id();
        assert!(
            is_access_preview_id(&id),
            "case {case:?}: expected an AccessPreviewId-shaped id, got {id:?}"
        );
        assert!(
            ids.insert(id.clone()),
            "case {case:?}: id {id:?} was already minted for another preview"
        );

        // Read back straight away: the analysis is not instantaneous, so the preview is
        // still CREATING and carries no statusReason (which explains a failure).
        let creating = emulator.get_access_preview(&id, &analyzer).await;
        assert_eq!(
            creating.status, 200,
            "case {case:?}: expected GetAccessPreview to succeed, got {creating:?}"
        );
        let preview = creating.preview();
        assert_eq!(
            preview.get("status").and_then(Value::as_str),
            Some("CREATING"),
            "case {case:?}: a preview read before its analysis is due must be CREATING, got {preview:?}"
        );
        assert!(
            preview.get("statusReason").is_none_or(Value::is_null),
            "case {case:?}: a CREATING preview must carry no statusReason, got {preview:?}"
        );
        assert_eq!(
            preview.get("id").and_then(Value::as_str),
            Some(id.as_str()),
            "case {case:?}: the preview must report the id the create returned, got {preview:?}"
        );
        assert_eq!(
            preview.get("analyzerArn").and_then(Value::as_str),
            Some(analyzer.as_str()),
            "case {case:?}: the preview must report the analyzer it was created against, got {preview:?}"
        );
        assert!(
            preview
                .get("createdAt")
                .is_some_and(|created_at| !created_at.is_null()),
            "case {case:?}: the preview must report a createdAt, got {preview:?}"
        );
        assert_eq!(
            preview.get("configurations").map(without_nulls),
            Some(without_nulls(&configurations)),
            "case {case:?}: the preview must report the configurations the create proposed, got {preview:?}"
        );
        created.push((case, id, configurations));
    }

    // The clock owes every one of these previews its terminal status.
    tokio::time::sleep(ACCESS_PREVIEW_ANALYSIS_SPAN + SETTLE_MARGIN).await;

    for (case, id, configurations) in &created {
        let completed = emulator.get_access_preview(id, &analyzer).await;
        let preview = completed.preview();
        assert_eq!(
            preview.get("status").and_then(Value::as_str),
            Some("COMPLETED"),
            "case {case:?}: an analyzable configuration must complete, got {preview:?}"
        );
        assert!(
            preview.get("statusReason").is_none_or(Value::is_null),
            "case {case:?}: a COMPLETED preview must carry no statusReason, got {preview:?}"
        );
        assert_eq!(
            preview.get("configurations").map(without_nulls),
            Some(without_nulls(configurations)),
            "case {case:?}: the stored configurations must not change as the preview completes, got {preview:?}"
        );
        // A read is reporting only: reading again reports the same preview.
        let again = emulator.get_access_preview(id, &analyzer).await;
        assert_eq!(
            without_nulls(again.preview()),
            without_nulls(preview),
            "case {case:?}: a second read must report the same preview"
        );
    }
}

/// RULE: a proposed configuration the service cannot analyze — one keyed by something
/// that is not a resource ARN, or proposing a policy that is not a JSON policy document
/// — still creates a preview, and that preview resolves to `FAILED` with status reason
/// `INVALID_CONFIGURATION` instead of `COMPLETED`.
#[tokio::test]
async fn a_configuration_that_cannot_be_analyzed_fails_the_preview() {
    let emulator = Emulator::new();
    let analyzer = emulator.analyzer_arn("preview_analyzer", "ACCOUNT").await;
    let bucket = "arn:aws:s3:::preview-bucket";

    // (case, configurations, expected terminal status)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a well-formed bucket policy",
            json!({bucket: {"s3Bucket": {"bucketPolicy": public_policy(bucket)}}}),
            "COMPLETED",
        ),
        (
            "no policy document at all",
            json!({"arn:aws:ec2:us-east-1:000000000000:snapshot/snap-1": {
                "ebsSnapshot": {"groups": ["all"]},
            }}),
            "COMPLETED",
        ),
        (
            "a bucket policy that is not JSON",
            json!({bucket: {"s3Bucket": {"bucketPolicy": "{\"Statement\": "}}}),
            "FAILED",
        ),
        (
            "a bucket policy that is a JSON array, not a policy document",
            json!({bucket: {"s3Bucket": {"bucketPolicy": "[]"}}}),
            "FAILED",
        ),
        (
            "a queue policy that is a JSON string, not a policy document",
            json!({"arn:aws:sqs:us-east-1:000000000000:q": {
                "sqsQueue": {"queuePolicy": "\"Allow\""},
            }}),
            "FAILED",
        ),
        (
            "an empty queue policy",
            json!({"arn:aws:sqs:us-east-1:000000000000:q": {"sqsQueue": {"queuePolicy": ""}}}),
            "FAILED",
        ),
        (
            "one malformed key policy among several",
            json!({"arn:aws:kms:us-east-1:000000000000:key/abcd": {"kmsKey": {"keyPolicies": {
                "default": scoped_policy("000000000000"),
                "broken": "not a policy",
            }}}}),
            "FAILED",
        ),
        (
            "a malformed access point policy under a well-formed bucket policy",
            json!({bucket: {"s3Bucket": {
                "bucketPolicy": public_policy(bucket),
                "accessPoints": {
                    "arn:aws:s3:us-east-1:000000000000:accesspoint/p": {
                        "accessPointPolicy": "}{",
                    },
                },
            }}}),
            "FAILED",
        ),
        (
            "a configurations key that is not a resource ARN",
            json!({"preview-bucket": {"s3Bucket": {"bucketPolicy": public_policy(bucket)}}}),
            "FAILED",
        ),
        (
            "a configurations key missing the resource segment of an ARN",
            json!({"arn:aws:s3:::": {"s3Bucket": {"bucketPolicy": public_policy(bucket)}}}),
            "FAILED",
        ),
    ];

    let mut created: Vec<(&str, String, &str)> = Vec::new();
    for (case, configurations, expected) in cases {
        let create = emulator
            .create_access_preview(json!({
                "analyzerArn": analyzer,
                "configurations": configurations,
            }))
            .await;
        assert_eq!(
            create.status, 200,
            "case {case:?}: the create is accepted whether or not the configuration analyzes, got {create:?}"
        );
        created.push((case, create.preview_id(), expected));
    }

    tokio::time::sleep(ACCESS_PREVIEW_ANALYSIS_SPAN + SETTLE_MARGIN).await;

    for (case, id, expected) in created {
        let reply = emulator.get_access_preview(&id, &analyzer).await;
        let preview = reply.preview();
        let status = preview.get("status").and_then(Value::as_str);
        assert_eq!(
            status,
            Some(expected),
            "case {case:?}: expected status {expected}, got {preview:?}"
        );
        let reason = preview
            .get("statusReason")
            .filter(|reason| !reason.is_null())
            .and_then(|reason| reason.get("code"))
            .and_then(Value::as_str);
        if expected == "FAILED" {
            assert_eq!(
                reason,
                Some("INVALID_CONFIGURATION"),
                "case {case:?}: a configuration that cannot be analyzed must report INVALID_CONFIGURATION, got {preview:?}"
            );
        } else {
            assert_eq!(
                reason, None,
                "case {case:?}: a completed preview explains nothing, got {preview:?}"
            );
        }
    }
}

/// RULE: an access preview belongs to the analyzer it was created against — a read is
/// resolved by `analyzerArn` AND `accessPreviewId`, so an unknown analyzer, an id no
/// analyzer minted, an id minted under a different analyzer, and an id whose analyzer
/// has been deleted are all `ResourceNotFoundException` (HTTP 404).
#[tokio::test]
async fn an_access_preview_is_scoped_to_the_analyzer_it_was_created_against() {
    let emulator = Emulator::new();
    let first = emulator.analyzer_arn("first_analyzer", "ACCOUNT").await;
    let second = emulator.analyzer_arn("second_analyzer", "ACCOUNT").await;
    let bucket = "arn:aws:s3:::preview-bucket";
    let configurations = json!({bucket: {"s3Bucket": {"bucketPolicy": public_policy(bucket)}}});

    let id = emulator
        .create_access_preview(json!({
            "analyzerArn": first,
            "configurations": configurations,
        }))
        .await
        .preview_id();

    // (case, id, analyzerArn) — every row names something this account does not hold.
    let unknown_analyzer = format!(
        "arn:aws:access-analyzer:{REGION}:000000000000:analyzer/no_such_analyzer"
    );
    let cases: Vec<(&str, String, String)> = vec![
        (
            "an id under an analyzer that does not exist",
            id.clone(),
            unknown_analyzer.clone(),
        ),
        (
            "an id this analyzer never minted",
            "00000000-0000-4000-8000-000000000000".to_string(),
            first.clone(),
        ),
        (
            "an id minted under a different analyzer",
            id.clone(),
            second.clone(),
        ),
    ];
    for (case, preview_id, analyzer_arn) in cases {
        let reply = emulator.get_access_preview(&preview_id, &analyzer_arn).await;
        assert_eq!(
            (reply.status, reply.code()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected a 404 ResourceNotFoundException, got {reply:?}"
        );
    }

    // The preview the first analyzer DOES hold still resolves.
    let found = emulator.get_access_preview(&id, &first).await;
    assert_eq!(
        found.status, 200,
        "the analyzer that minted the preview still resolves it, got {found:?}"
    );

    // A create against an analyzer that does not exist is refused the same way.
    let orphan = emulator
        .create_access_preview(json!({
            "analyzerArn": unknown_analyzer,
            "configurations": configurations,
        }))
        .await;
    assert_eq!(
        (orphan.status, orphan.code()),
        (404, "ResourceNotFoundException"),
        "a preview for an analyzer that does not exist is refused, got {orphan:?}"
    );

    // No preview outlives its analyzer: previews are stored on the analyzer record.
    let deleted = emulator.delete_analyzer("first_analyzer").await;
    assert_eq!(
        deleted.status, 200,
        "DeleteAnalyzer must succeed, got {deleted:?}"
    );
    let gone = emulator.get_access_preview(&id, &first).await;
    assert_eq!(
        (gone.status, gone.code()),
        (404, "ResourceNotFoundException"),
        "a preview must not outlive the analyzer it previews for, got {gone:?}"
    );
}

/// RULE: `configurations` is documented as containing exactly one element, so a map of
/// any other size is refused with `ValidationException` (HTTP 400) and stores nothing.
#[tokio::test]
async fn an_access_preview_takes_exactly_one_configuration() {
    let emulator = Emulator::new();
    let analyzer = emulator.analyzer_arn("preview_analyzer", "ACCOUNT").await;
    let bucket = "arn:aws:s3:::preview-bucket";
    let queue = "arn:aws:sqs:us-east-1:000000000000:preview-queue";
    let topic = "arn:aws:sns:us-east-1:000000000000:preview-topic";
    let one = json!({"s3Bucket": {"bucketPolicy": public_policy(bucket)}});
    let two = json!({"sqsQueue": {"queuePolicy": scoped_policy("111111111111")}});
    let three = json!({"snsTopic": {"topicPolicy": scoped_policy("111111111111")}});

    // (case, configurations, expected status)
    let cases: Vec<(&str, Value, u16)> = vec![
        ("no configuration at all", json!({}), 400),
        ("exactly one configuration", json!({bucket: one.clone()}), 200),
        (
            "two configurations",
            json!({bucket: one.clone(), queue: two.clone()}),
            400,
        ),
        (
            "three configurations",
            json!({bucket: one, queue: two, topic: three}),
            400,
        ),
    ];

    for (case, configurations, expected) in cases {
        let reply = emulator
            .create_access_preview(json!({
                "analyzerArn": analyzer,
                "configurations": configurations,
            }))
            .await;
        assert_eq!(
            reply.status, expected,
            "case {case:?}: expected HTTP {expected}, got {reply:?}"
        );
        if expected == 400 {
            assert_eq!(
                reply.code(),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {reply:?}"
            );
        }
    }
}

/// RULE: a preview is only created against an account external-access analyzer that is
/// active — `CreateAccessPreview$analyzerArn` documents "an Account type and Active
/// status" — so an analyzer of any other `Type` is refused with `ValidationException`
/// (HTTP 400).
#[tokio::test]
async fn an_access_preview_requires_an_active_account_analyzer() {
    let emulator = Emulator::new();
    let bucket = "arn:aws:s3:::preview-bucket";
    let configurations = json!({bucket: {"s3Bucket": {"bucketPolicy": public_policy(bucket)}}});

    // Every `Type` the model's `Type` enum accepts, from
    // `smithy_query(query="operation", operation="CreateAnalyzer")`'s `type` member;
    // only the account external-access analyzer supports access previews.
    let types: &[(&str, u16)] = &[
        ("ACCOUNT", 200),
        ("ACCOUNT_INTERNAL_ACCESS", 400),
        ("ACCOUNT_UNUSED_ACCESS", 400),
        ("ORGANIZATION", 400),
        ("ORGANIZATION_INTERNAL_ACCESS", 400),
        ("ORGANIZATION_UNUSED_ACCESS", 400),
    ];

    for (r#type, expected) in types {
        let name = format!("analyzer_{}", r#type.to_lowercase());
        let analyzer = emulator.analyzer_arn(&name, r#type).await;
        let reply = emulator
            .create_access_preview(json!({
                "analyzerArn": analyzer,
                "configurations": configurations,
            }))
            .await;
        assert_eq!(
            reply.status, *expected,
            "case {type:?}: expected HTTP {expected}, got {reply:?}"
        );
        if *expected == 400 {
            assert_eq!(
                reply.code(),
                "ValidationException",
                "case {type:?}: expected a ValidationException, got {reply:?}"
            );
        } else {
            assert!(
                is_access_preview_id(&reply.preview_id()),
                "case {type:?}: expected a minted id, got {reply:?}"
            );
        }
    }
}

/// RULE: `clientToken` makes the create idempotent — the SAME token answers with the
/// id the first call minted and previews nothing new, while any other token (or no
/// token) mints a new preview.
#[tokio::test]
async fn repeating_the_client_token_answers_with_the_first_preview() {
    let emulator = Emulator::new();
    let analyzer = emulator.analyzer_arn("preview_analyzer", "ACCOUNT").await;
    let bucket = "arn:aws:s3:::preview-bucket";
    let configurations = json!({bucket: {"s3Bucket": {"bucketPolicy": public_policy(bucket)}}});
    let request = |token: Option<&str>| match token {
        Some(token) => json!({
            "analyzerArn": analyzer,
            "configurations": configurations,
            "clientToken": token,
        }),
        None => json!({"analyzerArn": analyzer, "configurations": configurations}),
    };

    let first = emulator
        .create_access_preview(request(Some("token-one")))
        .await
        .preview_id();
    let retried = emulator
        .create_access_preview(request(Some("token-one")))
        .await
        .preview_id();
    assert_eq!(
        retried, first,
        "case retried token: a repeated clientToken must answer with the first id, got {retried:?} after {first:?}"
    );

    // (case, token) — every row is a DIFFERENT create and mints its own preview.
    let mut minted: BTreeSet<String> = BTreeSet::from([first]);
    for (case, token) in [
        ("another token", Some("token-two")),
        ("a third token", Some("token-three")),
        ("no token at all", None),
        ("no token at all, again", None),
    ] {
        let id = emulator.create_access_preview(request(token)).await.preview_id();
        assert!(
            minted.insert(id.clone()),
            "case {case:?}: expected a new preview, but id {id:?} was already minted"
        );
    }
}

/// RULE: the per-analyzer preview store is bounded — past the ceiling the create is
/// refused with the quota error the operation models (HTTP 402), not by growing the
/// store.
#[tokio::test]
async fn the_access_preview_store_is_bounded() {
    let emulator = Emulator::new();
    let analyzer = emulator.analyzer_arn("preview_analyzer", "ACCOUNT").await;
    let bucket = "arn:aws:s3:::preview-bucket";
    let body = json!({
        "analyzerArn": analyzer,
        "configurations": {bucket: {"s3Bucket": {"bucketPolicy": public_policy(bucket)}}},
    });

    for index in 0..MAX_ACCESS_PREVIEWS_PER_ANALYZER {
        let reply = emulator.create_access_preview(body.clone()).await;
        assert_eq!(
            reply.status, 200,
            "case preview {index}: expected the create to succeed below the ceiling, got {reply:?}"
        );
    }
    let refused = emulator.create_access_preview(body).await;
    assert_eq!(
        (refused.status, refused.code()),
        (402, "ServiceQuotaExceededException"),
        "case past the ceiling: expected a 402 ServiceQuotaExceededException, got {refused:?}"
    );
}
