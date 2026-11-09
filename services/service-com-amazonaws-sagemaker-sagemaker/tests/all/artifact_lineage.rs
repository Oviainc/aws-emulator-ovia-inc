// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's ML Lineage artifacts: `CreateArtifact` and the
//! `DescribeArtifact` that reads one back.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the ARN
//! form and its freshness for every Region; the create/describe round trip for every
//! member and every shape of value (absent, empty, unicode, control bytes, the published
//! maxima); the refusal of an empty property value; the per-Region uniqueness of
//! `ArtifactName`; and the not-found refusal for an ARN nothing created.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The name of the lineage group every artifact of a Region belongs to, read from the
/// crate's own derivation so the suite pins the name the SERVICE computes for it rather
/// than a second spelling of it.
fn default_lineage_group(region: &str) -> String {
    service_com_amazonaws_sagemaker_sagemaker::state::default_lineage_group_name(region)
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

    /// Call `target` unsigned: no credential scope, so the ARN region falls back to the
    /// emulator default (`state::DEFAULT_REGION`).
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

/// `Source` as a reader reports it for an artifact created with `body`: the source the
/// create sent, with `SourceTypes` as the LIST the artifact holds — the empty list when
/// the create named none, because the member reports what the artifact holds rather than
/// what the request mentioned.
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

/// `Properties` as a reader reports it for an artifact created with `body`: the map the
/// create sent, or the EMPTY map when it sent none — the same rule as
/// [`reported_source`]'s `SourceTypes`.
fn reported_properties(body: &Value) -> Value {
    body.get("Properties").cloned().unwrap_or_else(|| json!({}))
}

/// RULE: an `ArtifactArn` is `arn:aws:sagemaker:<signed region>:<account>:artifact/<id>`,
/// where `<id>` is 32 lowercase hex digits the server mints fresh for every create.
///
/// The id's form is the one the modelled `ArtifactArn` pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:artifact/`) admits after the slash.
fn assert_artifact_arn(case: &str, arn: &str, region: &str) {
    let prefix = format!("arn:aws:sagemaker:{region}:{ACCOUNT}:artifact/");
    assert!(
        arn.starts_with(&prefix),
        "case {case:?}: expected an ARN starting {prefix:?}, got {arn:?}"
    );
    let id = &arn[prefix.len()..];
    assert!(
        id.len() == 32 && id.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "case {case:?}: expected a 32-digit lowercase hex artifact id, got {id:?}"
    );
}

/// A source URI of exactly `len` characters, so a case can sit on a published bound.
fn source_uri_of_len(len: usize) -> String {
    let head = "s3://bucket/";
    format!("{head}{}", "a".repeat(len - head.len()))
}

#[tokio::test]
async fn every_created_artifact_gets_a_fresh_arn_in_the_region_the_caller_signed_for() {
    let emulator = Emulator::new();

    // Rows differ along the Region (including the unsigned fallback) and along whether
    // the create names the artifact at all.
    let regions = ["us-east-1", "eu-west-2", "ap-southeast-1", "us-gov-west-1"];
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for region in regions {
        for named in [true, false] {
            let case = format!("{region}/named={named}");
            let mut body = json!({
                "Source": { "SourceUri": format!("s3://bucket/{region}/{named}.csv") },
                "ArtifactType": "DataSet",
            });
            if named {
                body["ArtifactName"] = Value::String(format!("art-{}", region.replace('-', "")));
            }
            let created = emulator.ok_in(region, "CreateArtifact", &body).await;
            let arn = str_field(&created, "ArtifactArn");
            assert_artifact_arn(&case, &arn, region);
            assert!(
                seen.insert(arn.clone()),
                "case {case:?}: every create must mint a distinct ARN, saw {arn:?} twice"
            );
        }
    }

    // An unsigned request carries no credential scope, so the ARN names the emulator's
    // default Region.
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
        "case unsigned: expected HTTP 200, got {status} with {created}"
    );
    assert_artifact_arn("unsigned", &str_field(&created, "ArtifactArn"), "us-east-1");

    // Two creates carrying the IDENTICAL request body still get different ARNs: the id is
    // the server's, never derived from the request.
    let twin = json!({
        "Source": { "SourceUri": "s3://bucket/twin.csv" },
        "ArtifactType": "DataSet",
    });
    let first = str_field(&emulator.ok("CreateArtifact", &twin).await, "ArtifactArn");
    let second = str_field(&emulator.ok("CreateArtifact", &twin).await, "ArtifactArn");
    assert_ne!(
        first, second,
        "case identical-bodies: each create must mint its own ARN"
    );
}

#[tokio::test]
async fn describe_artifact_replays_exactly_what_create_artifact_stored() {
    let emulator = Emulator::new();

    // Rows differ along what the rule generalises over: which optional members are sent,
    // the shape of the source URI (plain, unicode, control bytes, the 2048-character
    // modelled maximum), the size of `Properties` (absent, empty, one, the modelled
    // maximum of 30 with a 4096-character value), the number of `SourceTypes` (absent,
    // empty, two) and how much of `MetadataProperties` is filled in.
    let mut thirty_properties = Map::new();
    for index in 0..30 {
        thirty_properties.insert(
            format!("kéy-{index}"),
            Value::String(if index == 0 {
                // `ArtifactPropertyValue` is `len 0..4096`: the upper bound, exercised.
                "v".repeat(4096)
            } else {
                format!("value-{index}")
            }),
        );
    }

    let cases: Vec<(&str, Value)> = vec![
        (
            "minimal-unnamed",
            json!({
                "Source": { "SourceUri": "s3://bucket/minimal.csv" },
                "ArtifactType": "DataSet",
            }),
        ),
        (
            "every-member",
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
                "MetadataProperties": {
                    "CommitId": "0f1e2d3c",
                    "Repository": "git://example/repo",
                    "GeneratedBy": "pipeline-7",
                    "ProjectId": "p-123456789012",
                },
                "Tags": [{ "Key": "team", "Value": "ml" }],
            }),
        ),
        (
            "empty-artifact-type",
            json!({
                // `String256` is `len 0..256`, so the empty string is a legal type.
                "ArtifactName": "empty-artifact-type",
                "Source": { "SourceUri": "s3://bucket/empty-type.csv" },
                "ArtifactType": "",
            }),
        ),
        (
            "max-artifact-type",
            json!({
                "ArtifactName": "max-artifact-type",
                "Source": { "SourceUri": "s3://bucket/max-type.csv" },
                "ArtifactType": "T".repeat(256),
            }),
        ),
        (
            "empty-properties-map",
            json!({
                "ArtifactName": "empty-properties-map",
                "Source": { "SourceUri": "s3://bucket/empty-props.csv" },
                "ArtifactType": "DataSet",
                "Properties": {},
            }),
        ),
        (
            "empty-source-types-list",
            json!({
                "ArtifactName": "empty-source-types",
                "Source": { "SourceUri": "s3://bucket/empty-types.csv", "SourceTypes": [] },
                "ArtifactType": "DataSet",
            }),
        ),
        (
            "unicode-source-uri",
            json!({
                "ArtifactName": "unicode-source-uri",
                "Source": { "SourceUri": "s3://bucket/café/数据/😀.csv" },
                "ArtifactType": "DataSet",
                "Properties": { "libellé": "café" },
            }),
        ),
        (
            "control-bytes-source-uri",
            json!({
                "ArtifactName": "control-bytes-source-uri",
                "Source": { "SourceUri": "s3://bucket/\u{0}\u{1}\u{7f}/raw.bin" },
                "ArtifactType": "Model",
            }),
        ),
        (
            "percent-encoded-source-uri",
            json!({
                "ArtifactName": "percent-encoded",
                "Source": { "SourceUri": "s3://bucket/a%20b/c+d?q=1&r=2#frag" },
                "ArtifactType": "Model",
            }),
        ),
        (
            "max-source-uri",
            json!({
                // `SourceUri` is `len 1..2048`: both bounds are exercised, here the upper.
                "ArtifactName": "max-source-uri",
                "Source": { "SourceUri": source_uri_of_len(2048) },
                "ArtifactType": "DataSet",
            }),
        ),
        (
            "single-character-source-uri",
            json!({
                "ArtifactName": "min-source-uri",
                "Source": { "SourceUri": "s" },
                "ArtifactType": "DataSet",
            }),
        ),
        (
            "thirty-properties",
            json!({
                "ArtifactName": "thirty-properties",
                "Source": { "SourceUri": "s3://bucket/thirty.csv" },
                "ArtifactType": "DataSet",
                "Properties": Value::Object(thirty_properties),
            }),
        ),
        (
            "partial-metadata-properties",
            json!({
                "ArtifactName": "partial-metadata",
                "Source": { "SourceUri": "s3://bucket/partial.csv" },
                "ArtifactType": "DataSet",
                "MetadataProperties": { "CommitId": "deadbeef" },
            }),
        ),
    ];

    for (case, body) in &cases {
        let created = emulator.ok("CreateArtifact", body).await;
        let arn = str_field(&created, "ArtifactArn");
        assert_artifact_arn(case, &arn, "us-east-1");

        let read_body = json!({ "ArtifactArn": arn });
        let described = emulator.ok("DescribeArtifact", &read_body).await;

        assert_eq!(
            described.get("ArtifactArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: DescribeArtifact must replay the ARN CreateArtifact minted, \
             got {described:?}"
        );
        // Every member the output shape carries over from the input: the name and the type
        // as sent, and the two collections the artifact OWNS as the artifact holds them —
        // the empty one where the create named none.
        for member in ["ArtifactName", "ArtifactType"] {
            assert_eq!(
                described.get(member),
                body.get(member),
                "case {case:?}: {member} must round-trip exactly, got {described:?}"
            );
        }
        assert_eq!(
            described.get("Source"),
            Some(&reported_source(body)),
            "case {case:?}: Source, got {described:?}"
        );
        assert_eq!(
            described.get("Properties"),
            Some(&reported_properties(body)),
            "case {case:?}: Properties, got {described:?}"
        );
        assert_eq!(
            described.get("MetadataProperties"),
            body.get("MetadataProperties"),
            "case {case:?}: MetadataProperties must round-trip exactly, got {described:?}"
        );
        // `DescribeArtifactResponse` models no `Tags` member, so tags are never echoed.
        assert!(
            described.get("Tags").is_none(),
            "case {case:?}: the output shape models no Tags member, got {described:?}"
        );

        let created_at = described
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a CreationTime in {described:?}"));
        let modified_at = described
            .get("LastModifiedTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a LastModifiedTime in {described:?}")
            });
        assert_eq!(
            created_at, modified_at,
            "case {case:?}: a never-updated artifact was last modified when it was created"
        );
        // The emulator models no Studio user profile and no IAM identity details, so the
        // user context is reported with no members rather than invented ones.
        for member in ["CreatedBy", "LastModifiedBy"] {
            assert_eq!(
                described.get(member),
                Some(&json!({})),
                "case {case:?}: {member} must be the empty user context, got {described:?}"
            );
        }
        assert_eq!(
            described.get("LineageGroupArn").and_then(Value::as_str),
            Some(
                format!(
                    "arn:aws:sagemaker:us-east-1:{ACCOUNT}:lineage-group/{}",
                    default_lineage_group("us-east-1")
                )
                .as_str()
            ),
            "case {case:?}: the artifact belongs to the Region's default lineage group, \
             got {described:?}"
        );

        // A read changes nothing a later caller sees: the second describe is identical.
        let again = emulator.ok("DescribeArtifact", &read_body).await;
        assert_eq!(
            again, described,
            "case {case:?}: DescribeArtifact must be stable across reads"
        );
    }
}

#[tokio::test]
async fn a_property_whose_value_is_empty_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();

    // Rows differ along where the empty value sits (alone, beside filled entries, in a
    // full 30-entry map) and along the key it is filed under.
    let mut thirty_with_one_empty = Map::new();
    for index in 0..30 {
        thirty_with_one_empty.insert(
            format!("key-{index}"),
            Value::String(if index == 17 {
                String::new()
            } else {
                format!("value-{index}")
            }),
        );
    }

    let refused: Vec<(&str, Value)> = vec![
        ("only-entry-empty", json!({ "stage": "" })),
        (
            "empty-beside-filled",
            json!({ "stage": "train", "rows": "", "owner": "ml" }),
        ),
        ("empty-key-and-value", json!({ "": "" })),
        ("empty-in-a-full-map", Value::Object(thirty_with_one_empty)),
    ];

    for (case, properties) in &refused {
        let name = format!("refused-{}", case.replace('-', ""));
        let (status, parsed) = emulator
            .call(
                "CreateArtifact",
                &json!({
                    "ArtifactName": name,
                    "Source": { "SourceUri": format!("s3://bucket/{case}.csv") },
                    "ArtifactType": "DataSet",
                    "Properties": properties,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an empty property value must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case:?}: error code, got {parsed:?}"
        );
        assert!(
            message(&parsed).contains("'properties'"),
            "case {case:?}: the refusal must name the member it refused, got {parsed:?}"
        );

        // Nothing was stored: the name the refused create asked for is still free, so a
        // later create may take it.
        let created = emulator
            .ok(
                "CreateArtifact",
                &json!({
                    "ArtifactName": name,
                    "Source": { "SourceUri": format!("s3://bucket/{case}-retry.csv") },
                    "ArtifactType": "DataSet",
                }),
            )
            .await;
        assert_artifact_arn(case, &str_field(&created, "ArtifactArn"), "us-east-1");
    }

    // The rule is about EMPTY values only: any non-empty value inside the modelled
    // bounds is accepted, including one that is only whitespace.
    let accepted: Vec<(&str, Value)> = vec![
        ("single-space", json!({ "stage": " " })),
        ("single-character", json!({ "stage": "t" })),
        ("maximum-length", json!({ "stage": "v".repeat(4096) })),
        ("empty-key-filled-value", json!({ "": "unnamed" })),
    ];
    for (case, properties) in &accepted {
        let body = json!({
            "Source": { "SourceUri": format!("s3://bucket/ok-{case}.csv") },
            "ArtifactType": "DataSet",
            "Properties": properties,
        });
        let created = emulator.ok("CreateArtifact", &body).await;
        let described = emulator
            .ok(
                "DescribeArtifact",
                &json!({ "ArtifactArn": str_field(&created, "ArtifactArn") }),
            )
            .await;
        assert_eq!(
            described.get("Properties"),
            body.get("Properties"),
            "case {case:?}: a non-empty property value must round-trip, got {described:?}"
        );
    }
}

#[tokio::test]
async fn requests_outside_the_modelled_constraints_are_refused() {
    let emulator = Emulator::new();

    // Rows: one past each published bound of the members this operation reads —
    // `SourceUri` len 1..2048, `ArtifactProperties` len 0..30, `ArtifactPropertyValue`
    // len 0..4096, `String256` len 0..256, `ExperimentEntityName` len 1..120 and its
    // pattern.
    let mut thirty_one = Map::new();
    for index in 0..31 {
        thirty_one.insert(format!("key-{index}"), Value::String("v".to_string()));
    }

    let cases: Vec<(&str, Value)> = vec![
        (
            "source-uri-empty",
            json!({
                "Source": { "SourceUri": "" },
                "ArtifactType": "DataSet",
            }),
        ),
        (
            "source-uri-one-past-the-maximum",
            json!({
                "Source": { "SourceUri": source_uri_of_len(2049) },
                "ArtifactType": "DataSet",
            }),
        ),
        (
            "artifact-type-one-past-the-maximum",
            json!({
                "Source": { "SourceUri": "s3://bucket/long-type.csv" },
                "ArtifactType": "T".repeat(257),
            }),
        ),
        (
            "thirty-one-properties",
            json!({
                "Source": { "SourceUri": "s3://bucket/too-many-props.csv" },
                "ArtifactType": "DataSet",
                "Properties": Value::Object(thirty_one),
            }),
        ),
        (
            "property-value-one-past-the-maximum",
            json!({
                "Source": { "SourceUri": "s3://bucket/long-value.csv" },
                "ArtifactType": "DataSet",
                "Properties": { "stage": "v".repeat(4097) },
            }),
        ),
        (
            "artifact-name-one-past-the-maximum",
            json!({
                "ArtifactName": "n".repeat(121),
                "Source": { "SourceUri": "s3://bucket/long-name.csv" },
                "ArtifactType": "DataSet",
            }),
        ),
        (
            "artifact-name-outside-the-pattern",
            json!({
                "ArtifactName": "-leading-hyphen",
                "Source": { "SourceUri": "s3://bucket/bad-name.csv" },
                "ArtifactType": "DataSet",
            }),
        ),
        (
            "source-absent",
            json!({
                "ArtifactType": "DataSet",
            }),
        ),
        (
            "artifact-type-absent",
            json!({
                "Source": { "SourceUri": "s3://bucket/no-type.csv" },
            }),
        ),
        (
            "describe-without-an-arn",
            json!({}),
        ),
    ];

    for (case, body) in &cases {
        let target = if *case == "describe-without-an-arn" {
            "DescribeArtifact"
        } else {
            "CreateArtifact"
        };
        let (status, parsed) = emulator.call(target, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a request outside the modelled constraints must be refused, \
             got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case:?}: error code, got {parsed:?}"
        );
    }
}

#[tokio::test]
async fn an_artifact_name_already_in_use_in_the_region_is_refused_and_keeps_the_stored_artifact() {
    let emulator = Emulator::new();

    let first = emulator
        .ok(
            "CreateArtifact",
            &json!({
                "ArtifactName": "taken",
                "Source": { "SourceUri": "s3://bucket/first.csv" },
                "ArtifactType": "DataSet",
                "Properties": { "stage": "first" },
            }),
        )
        .await;
    let first_arn = str_field(&first, "ArtifactArn");

    // Rows: a second create under the same name, whatever else differs about it.
    let cases: Vec<(&str, Value)> = vec![
        (
            "same-name-same-source",
            json!({
                "ArtifactName": "taken",
                "Source": { "SourceUri": "s3://bucket/first.csv" },
                "ArtifactType": "DataSet",
            }),
        ),
        (
            "same-name-other-source-and-type",
            json!({
                "ArtifactName": "taken",
                "Source": { "SourceUri": "s3://bucket/second.csv" },
                "ArtifactType": "Model",
                "Properties": { "stage": "second" },
            }),
        ),
    ];

    for (case, body) in &cases {
        let (status, parsed) = emulator.call("CreateArtifact", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a name already in use must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case:?}: error code, got {parsed:?}"
        );
        assert!(
            message(&parsed).contains(&first_arn),
            "case {case:?}: the refusal must name the artifact already holding it, \
             got {parsed:?}"
        );
    }

    // The stored artifact is untouched by the refused creates.
    let described = emulator
        .ok("DescribeArtifact", &json!({ "ArtifactArn": first_arn }))
        .await;
    assert_eq!(
        described.get("Properties"),
        Some(&json!({ "stage": "first" })),
        "case after-refusals: the stored artifact must be unchanged, got {described:?}"
    );

    // The name is unique per Region, so the same name is free in another one.
    for region in ["eu-west-2", "ap-southeast-1"] {
        let created = emulator
            .ok_in(
                region,
                "CreateArtifact",
                &json!({
                    "ArtifactName": "taken",
                    "Source": { "SourceUri": "s3://bucket/first.csv" },
                    "ArtifactType": "DataSet",
                }),
            )
            .await;
        assert_artifact_arn(region, &str_field(&created, "ArtifactArn"), region);
    }
}

#[tokio::test]
async fn describing_an_artifact_arn_nothing_created_is_refused_with_resource_not_found() {
    let emulator = Emulator::new();

    let created = emulator
        .ok(
            "CreateArtifact",
            &json!({
                "ArtifactName": "present",
                "Source": { "SourceUri": "s3://bucket/present.csv" },
                "ArtifactType": "DataSet",
            }),
        )
        .await;
    let present = str_field(&created, "ArtifactArn");

    // Rows: ARNs inside the modelled pattern that name no stored artifact — an id of the
    // minted shape, a name-shaped id, another Region, and another partition.
    let cases = [
        (
            "unknown-hex-id",
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:artifact/{}", "0".repeat(32)),
        ),
        (
            "name-shaped-id",
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:artifact/not-an-id"),
        ),
        (
            "same-id-other-region",
            present.replace("us-east-1", "eu-west-2"),
        ),
        (
            "same-id-other-partition",
            present.replace("arn:aws:", "arn:aws-us-gov:"),
        ),
    ];

    for (case, arn) in &cases {
        let (status, parsed) = emulator
            .call("DescribeArtifact", &json!({ "ArtifactArn": arn }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an artifact nothing created must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {case:?}: the refusal must use the operation's modelled not-found error, \
             got {parsed:?}"
        );
        assert!(
            message(&parsed).contains(arn.as_str()),
            "case {case:?}: the refusal must name the ARN it could not find, got {parsed:?}"
        );
    }

    // The artifact that does exist is still readable.
    let described = emulator
        .ok("DescribeArtifact", &json!({ "ArtifactArn": present }))
        .await;
    assert_eq!(
        described.get("ArtifactName").and_then(Value::as_str),
        Some("present"),
        "case present: the stored artifact must still be readable, got {described:?}"
    );
}
