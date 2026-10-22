// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's inference-deployment lifecycle:
//! model -> endpoint config -> endpoint, plus `ListModels` and the two deletes.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the ARN
//! form for every resource kind, region and name shape; the create/describe round trip
//! for every optional member; the not-found refusal for every reader; and `ListModels`
//! for every documented sort, filter and page size.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Long enough to pass `state::ENDPOINT_IN_SERVICE_AFTER` (300 ms).
const PAST_IN_SERVICE_DUE: Duration = Duration::from_millis(600);

/// Spacing between creates, so `CreationTime` ordering is unambiguous and a filter
/// bound can be placed strictly between two models.
const CREATE_SPACING: Duration = Duration::from_millis(5);

/// One emulator: the shared engine plus the SageMaker service mounted on the router.
///
/// Shared with this crate's other suites (`use super::inference_deployment_lifecycle::*`)
/// so every wire test drives the same router the production one assembles, rather than
/// each suite growing its own copy.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
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
    pub(crate) async fn call_in(&self, region: &str, target: &str, body: &Value) -> (u16, Value) {
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

    /// Call `target` unsigned: no credential scope, so the request routes by
    /// `X-Amz-Target` alone and the ARN region falls back to the emulator default.
    pub(crate) async fn call_unsigned(&self, target: &str, body: &Value) -> (u16, Value) {
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

    pub(crate) async fn call(&self, target: &str, body: &Value) -> (u16, Value) {
        self.call_in("us-east-1", target, body).await
    }

    /// Call `target` and require a 200, returning the parsed output.
    pub(crate) async fn ok(&self, target: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(target, body).await;
        assert_eq!(
            status, 200,
            "case {target} {body}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }
}

/// The `__type` an error body carries, with any `namespace#` prefix stripped.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

pub(crate) fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub(crate) fn str_field(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string member {member} in {body}"))
        .to_string()
}

fn primary_container(image: &str) -> Value {
    json!({ "Image": image })
}

/// A minimal one-variant configuration body naming `model`.
fn one_variant_config(name: &str, model: &str) -> Value {
    json!({
        "EndpointConfigName": name,
        "ProductionVariants": [{
            "VariantName": "AllTraffic",
            "ModelName": model,
            "InitialInstanceCount": 1,
            "InstanceType": "ml.m5.large",
        }],
    })
}

/// RULE: every ARN this service mints is
/// `arn:aws:sagemaker:<signed region>:<account>:<kind>/<name>`, for each of the three
/// resource kinds, whatever the name's shape and whatever region the caller signed for
/// (and the emulator default when the request is unsigned).
#[tokio::test]
async fn minted_arns_name_the_resource_kind_the_signed_region_and_the_stored_name() {
    // Rows differ along the axes the rule generalises over: resource kind, region, and
    // the name's shape (short, digits, hyphens, and the 63-character modelled maximum).
    let names = [
        "m",
        "model1",
        "demo-model-with-hyphens",
        // `ModelName` / `EndpointConfigName` / `EndpointName` are all `len 0..63`
        // (smithy_query operation=CreateModel), so this row sits on the upper bound.
        "a234567890123456789012345678901234567890123456789012345678901b",
    ];
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    for region in regions {
        for (index, name) in names.iter().enumerate() {
            let emulator = Emulator::new();
            let case = format!("region={region} name={name}");

            let created = emulator
                .call_in(
                    region,
                    "CreateModel",
                    &json!({ "ModelName": name, "PrimaryContainer": primary_container("demo:1") }),
                )
                .await;
            assert_eq!(created.0, 200, "case {case}: CreateModel {:?}", created.1);
            assert_eq!(
                str_field(&created.1, "ModelArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:model/{name}"),
                "case {case}: ModelArn, got {:?}",
                created.1
            );

            let config_name = format!("cfg{index}");
            let config = emulator
                .call_in(
                    region,
                    "CreateEndpointConfig",
                    &one_variant_config(&config_name, name),
                )
                .await;
            assert_eq!(config.0, 200, "case {case}: CreateEndpointConfig {:?}", config.1);
            assert_eq!(
                str_field(&config.1, "EndpointConfigArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:endpoint-config/{config_name}"),
                "case {case}: EndpointConfigArn, got {:?}",
                config.1
            );

            let endpoint_name = format!("ep{index}");
            let endpoint = emulator
                .call_in(
                    region,
                    "CreateEndpoint",
                    &json!({ "EndpointName": endpoint_name, "EndpointConfigName": config_name }),
                )
                .await;
            assert_eq!(endpoint.0, 200, "case {case}: CreateEndpoint {:?}", endpoint.1);
            assert_eq!(
                str_field(&endpoint.1, "EndpointArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:endpoint/{endpoint_name}"),
                "case {case}: EndpointArn, got {:?}",
                endpoint.1
            );
        }
    }

    // A credential scope whose region is not a region token cannot enter an ARN (the
    // region component is positional), so the emulator default stands in.
    for malformed in ["us east 1", "US-EAST-1", "us:east/1"] {
        let emulator = Emulator::new();
        let (status, body) = emulator
            .call_in(
                malformed,
                "CreateModel",
                &json!({ "ModelName": "odd-region", "PrimaryContainer": primary_container("demo:1") }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case malformed-region={malformed}: CreateModel, got {status} {body}"
        );
        assert_eq!(
            str_field(&body, "ModelArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model/odd-region"),
            "case malformed-region={malformed}: ModelArn must fall back to the default \
             region, got {body:?}"
        );
    }

    // An unsigned request names no region, so the ARN carries the emulator's default.
    let emulator = Emulator::new();
    let (status, body) = emulator
        .call_unsigned(
            "CreateModel",
            &json!({ "ModelName": "unsigned-model", "PrimaryContainer": primary_container("demo:1") }),
        )
        .await;
    assert_eq!(status, 200, "case unsigned: CreateModel, got {status} {body}");
    assert_eq!(
        str_field(&body, "ModelArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model/unsigned-model"),
        "case unsigned: ModelArn, got {body:?}"
    );
}

/// RULE: `DescribeModel` replays every member `CreateModel` stored, present members and
/// absent ones alike, and the `CreationTime` it reports is the one recorded at create.
#[tokio::test]
async fn describe_model_replays_exactly_what_create_model_stored() {
    // Rows differ along container count, VPC configuration, network isolation, the
    // multi-container execution mode, the role, and tag count.
    let cases: Vec<(&str, Value)> = vec![
        (
            "primary-container-only",
            json!({ "PrimaryContainer": primary_container("demo:1") }),
        ),
        (
            "single-container-pipeline",
            json!({ "Containers": [primary_container("first:1")] }),
        ),
        (
            "three-container-pipeline",
            json!({
                "Containers": [
                    primary_container("first:1"),
                    { "Image": "second:2", "Environment": { "K": "V" } },
                    { "Image": "third:3", "ContainerHostname": "third" },
                ],
                "InferenceExecutionConfig": { "Mode": "Direct" },
            }),
        ),
        (
            "vpc-and-isolation",
            json!({
                "PrimaryContainer": primary_container("demo:1"),
                "ExecutionRoleArn": "arn:aws:iam::000000000000:role/SMRole",
                "VpcConfig": {
                    "SecurityGroupIds": ["sg-1", "sg-2"],
                    "Subnets": ["subnet-1"],
                },
                "EnableNetworkIsolation": true,
            }),
        ),
        (
            "isolation-explicitly-off-with-tags",
            json!({
                "PrimaryContainer": primary_container("demo:1"),
                "EnableNetworkIsolation": false,
                "Tags": [{ "Key": "team", "Value": "ml" }, { "Key": "env", "Value": "test" }],
            }),
        ),
    ];

    for (case, extra) in cases {
        let emulator = Emulator::new();
        let mut body = json!({ "ModelName": case });
        for (key, value) in extra.as_object().expect("a JSON object of extra members") {
            body[key] = value.clone();
        }

        let created = emulator.ok("CreateModel", &body).await;
        let described = emulator
            .ok("DescribeModel", &json!({ "ModelName": case }))
            .await;

        assert_eq!(
            str_field(&described, "ModelName"),
            case,
            "case {case}: ModelName, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "ModelArn"),
            str_field(&created, "ModelArn"),
            "case {case}: DescribeModel must replay the ARN CreateModel minted"
        );
        assert!(
            described.get("CreationTime").is_some_and(Value::is_number),
            "case {case}: CreationTime must be a stored timestamp, got {described:?}"
        );

        // Every member the request carried must come back unchanged; `Tags` is stored
        // but not part of DescribeModel's modelled output, so it is excluded here.
        for member in ["PrimaryContainer", "Containers", "InferenceExecutionConfig",
                       "ExecutionRoleArn", "VpcConfig", "EnableNetworkIsolation"] {
            assert_eq!(
                described.get(member),
                body.get(member),
                "case {case}: {member} must round-trip, got {described:?}"
            );
        }

        // A second read sees the same values: a read changes nothing.
        let again = emulator
            .ok("DescribeModel", &json!({ "ModelName": case }))
            .await;
        assert_eq!(
            again, described,
            "case {case}: a second DescribeModel must answer identically"
        );
    }
}

/// RULE: `DescribeEndpointConfig` replays every member `CreateEndpointConfig` stored,
/// for one variant and for the modelled maximum of ten, instance-backed or serverless.
#[tokio::test]
async fn describe_endpoint_config_replays_exactly_what_create_stored() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateModel",
            &json!({ "ModelName": "shared-model", "PrimaryContainer": primary_container("demo:1") }),
        )
        .await;

    // `ProductionVariants` is `len 1..10` (smithy_query operation=CreateEndpointConfig),
    // so the rows sit on both published bounds plus a serverless variant in between.
    let ten_variants: Vec<Value> = (0..10)
        .map(|index| {
            json!({
                "VariantName": format!("v{index}"),
                "ModelName": "shared-model",
                "InitialInstanceCount": index + 1,
                "InstanceType": "ml.m5.large",
                "InitialVariantWeight": 0.5,
            })
        })
        .collect();

    let cases: Vec<(&str, Value)> = vec![
        (
            "one-variant",
            json!({
                "ProductionVariants": [{
                    "VariantName": "AllTraffic",
                    "ModelName": "shared-model",
                    "InitialInstanceCount": 2,
                    "InstanceType": "ml.c5.xlarge",
                }],
            }),
        ),
        (
            "ten-variants-with-kms-and-capture",
            json!({
                "ProductionVariants": ten_variants,
                "KmsKeyId": "alias/sagemaker",
                "DataCaptureConfig": {
                    "EnableCapture": true,
                    "InitialSamplingPercentage": 25,
                    "DestinationS3Uri": "s3://bucket/capture",
                    "CaptureOptions": [{ "CaptureMode": "Input" }],
                },
            }),
        ),
        (
            "serverless-variant",
            json!({
                "ProductionVariants": [{
                    "VariantName": "Serverless",
                    "ModelName": "shared-model",
                    "ServerlessConfig": { "MemorySizeInMB": 2048, "MaxConcurrency": 5 },
                }],
                "EnableNetworkIsolation": true,
            }),
        ),
    ];

    for (case, extra) in cases {
        let mut body = json!({ "EndpointConfigName": case });
        for (key, value) in extra.as_object().expect("a JSON object of extra members") {
            body[key] = value.clone();
        }

        let created = emulator.ok("CreateEndpointConfig", &body).await;
        let described = emulator
            .ok(
                "DescribeEndpointConfig",
                &json!({ "EndpointConfigName": case }),
            )
            .await;

        assert_eq!(
            str_field(&described, "EndpointConfigArn"),
            str_field(&created, "EndpointConfigArn"),
            "case {case}: DescribeEndpointConfig must replay the ARN create minted"
        );
        for member in ["ProductionVariants", "KmsKeyId", "DataCaptureConfig",
                       "EnableNetworkIsolation"] {
            assert_eq!(
                described.get(member),
                body.get(member),
                "case {case}: {member} must round-trip, got {described:?}"
            );
        }
        assert!(
            described.get("CreationTime").is_some_and(Value::is_number),
            "case {case}: CreationTime must be a stored timestamp, got {described:?}"
        );
    }
}

/// RULE: a reader or a delete naming a resource that is not in state is refused with
/// HTTP 400 `ValidationException` — never a 200 with empty members, and never a 501.
#[tokio::test]
async fn an_absent_resource_is_refused_with_a_400_validation_exception() {
    let emulator = Emulator::new();

    // Rows: every implemented operation that resolves a name, each with a name nothing
    // created, so the rule is asserted over the whole class rather than one reader.
    let cases: Vec<(&str, Value, &str)> = vec![
        ("DescribeModel", json!({ "ModelName": "absent" }), "model"),
        ("DeleteModel", json!({ "ModelName": "absent" }), "model"),
        (
            "DescribeEndpointConfig",
            json!({ "EndpointConfigName": "absent" }),
            "endpoint-config",
        ),
        (
            "DescribeEndpoint",
            json!({ "EndpointName": "absent" }),
            "endpoint",
        ),
        (
            "DeleteEndpoint",
            json!({ "EndpointName": "absent" }),
            "endpoint",
        ),
        (
            "CreateEndpoint",
            json!({ "EndpointName": "fresh", "EndpointConfigName": "absent" }),
            "endpoint-config",
        ),
        (
            "CreateEndpointConfig",
            one_variant_config("fresh-cfg", "absent"),
            "model",
        ),
    ];

    for (target, body, kind) in cases {
        let (status, parsed) = emulator.call(target, &body).await;
        assert_eq!(
            status, 400,
            "case {target}: expected HTTP 400 for an absent resource, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {target}: error code, got {parsed:?}"
        );
        assert!(
            message(&parsed).contains(&format!("{ACCOUNT}:{kind}/absent")),
            "case {target}: the refusal must name the {kind} it could not find, got {parsed:?}"
        );
    }
}

/// RULE: a create whose name is already in use is refused, so the stored resource is
/// never silently overwritten.
#[tokio::test]
async fn creating_a_name_that_is_already_in_use_is_refused_and_keeps_the_stored_resource() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateModel",
            &json!({ "ModelName": "dup", "PrimaryContainer": primary_container("first:1") }),
        )
        .await;
    emulator
        .ok("CreateEndpointConfig", &one_variant_config("dup-cfg", "dup"))
        .await;
    emulator
        .ok(
            "CreateEndpoint",
            &json!({ "EndpointName": "dup-ep", "EndpointConfigName": "dup-cfg" }),
        )
        .await;

    // Rows: each of the three create operations, re-run with the same name but a
    // DIFFERENT body, so a silent overwrite would be visible in the follow-up read.
    let cases: Vec<(&str, Value, &str, Value, &str)> = vec![
        (
            "CreateModel",
            json!({ "ModelName": "dup", "PrimaryContainer": primary_container("second:2") }),
            "DescribeModel",
            json!({ "ModelName": "dup" }),
            "model/dup",
        ),
        (
            "CreateEndpointConfig",
            json!({
                "EndpointConfigName": "dup-cfg",
                "ProductionVariants": [{
                    "VariantName": "Replacement",
                    "ModelName": "dup",
                    "InitialInstanceCount": 9,
                    "InstanceType": "ml.m5.24xlarge",
                }],
            }),
            "DescribeEndpointConfig",
            json!({ "EndpointConfigName": "dup-cfg" }),
            "endpoint-config/dup-cfg",
        ),
        (
            "CreateEndpoint",
            json!({ "EndpointName": "dup-ep", "EndpointConfigName": "dup-cfg" }),
            "DescribeEndpoint",
            json!({ "EndpointName": "dup-ep" }),
            "endpoint/dup-ep",
        ),
    ];

    for (target, body, reader, read_body, arn_tail) in cases {
        let before = emulator.ok(reader, &read_body).await;
        let (status, parsed) = emulator.call(target, &body).await;
        assert_eq!(
            status, 400,
            "case {target}: a name already in use must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {target}: error code, got {parsed:?}"
        );
        assert!(
            message(&parsed).contains(arn_tail),
            "case {target}: the refusal must name the existing resource, got {parsed:?}"
        );
        let after = emulator.ok(reader, &read_body).await;
        assert_eq!(
            after, before,
            "case {target}: the refused create must leave the stored resource untouched"
        );
    }
}

/// RULE: an endpoint reports the variant summary its configuration's variants imply —
/// `InitialVariantWeight` (defaulting to 1.0, per the member's model doc) as current and
/// desired weight, `InitialInstanceCount` as current and desired count, and a serverless
/// variant's config as the current and desired serverless config.
#[tokio::test]
async fn endpoint_production_variants_are_derived_from_the_stored_configuration() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateModel",
            &json!({ "ModelName": "pv-model", "PrimaryContainer": primary_container("demo:1") }),
        )
        .await;

    // Rows differ along: weight given vs defaulted, instance count, and instance-backed
    // vs serverless.
    let variants = json!([
        {
            "VariantName": "weighted",
            "ModelName": "pv-model",
            "InitialInstanceCount": 3,
            "InstanceType": "ml.m5.large",
            "InitialVariantWeight": 0.25,
        },
        {
            "VariantName": "defaulted-weight",
            "ModelName": "pv-model",
            "InitialInstanceCount": 1,
            "InstanceType": "ml.m5.large",
        },
        {
            "VariantName": "serverless",
            "ModelName": "pv-model",
            "ServerlessConfig": { "MemorySizeInMB": 1024, "MaxConcurrency": 2 },
        },
    ]);

    emulator
        .ok(
            "CreateEndpointConfig",
            &json!({
                "EndpointConfigName": "pv-cfg",
                "ProductionVariants": variants,
                "ShadowProductionVariants": [{
                    "VariantName": "shadow",
                    "ModelName": "pv-model",
                    "InitialInstanceCount": 2,
                    "InstanceType": "ml.m5.large",
                    "InitialVariantWeight": 0.1,
                }],
            }),
        )
        .await;
    emulator
        .ok(
            "CreateEndpoint",
            &json!({ "EndpointName": "pv-ep", "EndpointConfigName": "pv-cfg" }),
        )
        .await;

    let described = emulator
        .ok("DescribeEndpoint", &json!({ "EndpointName": "pv-ep" }))
        .await;
    let summaries = described
        .get("ProductionVariants")
        .and_then(Value::as_array)
        .expect("DescribeEndpoint must report ProductionVariants");

    let expected: Vec<(&str, f64, Option<i64>, Option<i64>)> = vec![
        ("weighted", 0.25, Some(3), None),
        ("defaulted-weight", 1.0, Some(1), None),
        ("serverless", 1.0, None, Some(1024)),
    ];
    assert_eq!(
        summaries.len(),
        expected.len(),
        "one summary per configured variant, got {summaries:?}"
    );

    for (index, (name, weight, count, memory)) in expected.into_iter().enumerate() {
        let summary = &summaries[index];
        let case = format!("variant {name}");
        assert_eq!(
            str_field(summary, "VariantName"),
            name,
            "case {case}: VariantName, got {summary:?}"
        );
        for member in ["CurrentWeight", "DesiredWeight"] {
            let actual = summary.get(member).and_then(Value::as_f64);
            assert_eq!(
                actual,
                Some(weight),
                "case {case}: {member}, got {summary:?}"
            );
        }
        for member in ["CurrentInstanceCount", "DesiredInstanceCount"] {
            assert_eq!(
                summary.get(member).and_then(Value::as_i64),
                count,
                "case {case}: {member}, got {summary:?}"
            );
        }
        for member in ["CurrentServerlessConfig", "DesiredServerlessConfig"] {
            assert_eq!(
                summary
                    .get(member)
                    .and_then(|config| config.get("MemorySizeInMB"))
                    .and_then(Value::as_i64),
                memory,
                "case {case}: {member}, got {summary:?}"
            );
        }
    }

    // The configuration's shadow variants are summarised the same way, and an endpoint
    // whose configuration carried none omits the member entirely.
    let shadow = described
        .get("ShadowProductionVariants")
        .and_then(Value::as_array)
        .expect("DescribeEndpoint must report the configured ShadowProductionVariants");
    assert_eq!(
        shadow.len(),
        1,
        "case shadow-variants: one summary per configured shadow variant, got {shadow:?}"
    );
    assert_eq!(
        str_field(&shadow[0], "VariantName"),
        "shadow",
        "case shadow-variants: VariantName, got {shadow:?}"
    );
    assert_eq!(
        shadow[0].get("DesiredInstanceCount").and_then(Value::as_i64),
        Some(2),
        "case shadow-variants: DesiredInstanceCount, got {shadow:?}"
    );

    emulator
        .ok(
            "CreateEndpointConfig",
            &one_variant_config("no-shadow-cfg", "pv-model"),
        )
        .await;
    emulator
        .ok(
            "CreateEndpoint",
            &json!({ "EndpointName": "no-shadow-ep", "EndpointConfigName": "no-shadow-cfg" }),
        )
        .await;
    let without_shadow = emulator
        .ok("DescribeEndpoint", &json!({ "EndpointName": "no-shadow-ep" }))
        .await;
    assert!(
        without_shadow.get("ShadowProductionVariants").is_none(),
        "case no-shadow-variants: the member must be omitted, got {without_shadow:?}"
    );
}

/// RULE: a newly created endpoint is reported `Creating`, and the `InService` transition
/// the clock owes becomes visible once its span has elapsed — without any call having to
/// make it happen, and stably thereafter.
#[tokio::test]
async fn an_endpoint_advances_from_creating_to_in_service_once_the_clock_owes_it() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateModel",
            &json!({ "ModelName": "wait-model", "PrimaryContainer": primary_container("demo:1") }),
        )
        .await;
    emulator
        .ok(
            "CreateEndpointConfig",
            &one_variant_config("wait-cfg", "wait-model"),
        )
        .await;
    emulator
        .ok(
            "CreateEndpoint",
            &json!({ "EndpointName": "wait-ep", "EndpointConfigName": "wait-cfg" }),
        )
        .await;

    let creating = emulator
        .ok("DescribeEndpoint", &json!({ "EndpointName": "wait-ep" }))
        .await;
    assert_eq!(
        str_field(&creating, "EndpointStatus"),
        "Creating",
        "case immediately-after-create: EndpointStatus, got {creating:?}"
    );
    assert_eq!(
        str_field(&creating, "EndpointConfigName"),
        "wait-cfg",
        "case immediately-after-create: EndpointConfigName, got {creating:?}"
    );

    tokio::time::sleep(PAST_IN_SERVICE_DUE).await;

    let settled = emulator
        .ok("DescribeEndpoint", &json!({ "EndpointName": "wait-ep" }))
        .await;
    assert_eq!(
        str_field(&settled, "EndpointStatus"),
        "InService",
        "case after-the-due-span: EndpointStatus, got {settled:?}"
    );
    let created_at = settled
        .get("CreationTime")
        .and_then(Value::as_f64)
        .expect("CreationTime");
    let modified_at = settled
        .get("LastModifiedTime")
        .and_then(Value::as_f64)
        .expect("LastModifiedTime");
    assert!(
        modified_at > created_at,
        "case after-the-due-span: LastModifiedTime {modified_at} must move past \
         CreationTime {created_at} when the status changes"
    );
    assert_eq!(
        settled.get("CreationTime"),
        creating.get("CreationTime"),
        "case after-the-due-span: CreationTime must not move, got {settled:?}"
    );

    let again = emulator
        .ok("DescribeEndpoint", &json!({ "EndpointName": "wait-ep" }))
        .await;
    assert_eq!(
        again, settled,
        "case repeated-read: a settled endpoint must read back identically"
    );
}

/// RULE: a delete removes the entry, so the reader refuses it afterwards and `ListModels`
/// stops listing it, while every sibling resource is untouched.
#[tokio::test]
async fn deleting_a_resource_removes_only_that_entry() {
    let emulator = Emulator::new();
    for name in ["keep-model", "drop-model"] {
        emulator
            .ok(
                "CreateModel",
                &json!({ "ModelName": name, "PrimaryContainer": primary_container("demo:1") }),
            )
            .await;
    }
    emulator
        .ok(
            "CreateEndpointConfig",
            &one_variant_config("del-cfg", "keep-model"),
        )
        .await;
    for name in ["keep-ep", "drop-ep"] {
        emulator
            .ok(
                "CreateEndpoint",
                &json!({ "EndpointName": name, "EndpointConfigName": "del-cfg" }),
            )
            .await;
    }

    // Rows: both deletes, each with the sibling that must survive it.
    let cases: Vec<(&str, Value, &str, Value, Value)> = vec![
        (
            "DeleteEndpoint",
            json!({ "EndpointName": "drop-ep" }),
            "DescribeEndpoint",
            json!({ "EndpointName": "drop-ep" }),
            json!({ "EndpointName": "keep-ep" }),
        ),
        (
            "DeleteModel",
            json!({ "ModelName": "drop-model" }),
            "DescribeModel",
            json!({ "ModelName": "drop-model" }),
            json!({ "ModelName": "keep-model" }),
        ),
    ];

    for (target, body, reader, gone_body, survivor_body) in cases {
        let deleted = emulator.ok(target, &body).await;
        assert_eq!(
            deleted,
            json!({}),
            "case {target}: the modelled output has no members, got {deleted:?}"
        );

        let (status, parsed) = emulator.call(reader, &gone_body).await;
        assert_eq!(
            status, 400,
            "case {target}: the deleted resource must be gone, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {target}: error code after delete, got {parsed:?}"
        );

        let survivor = emulator.ok(reader, &survivor_body).await;
        assert!(
            survivor.get("CreationTime").is_some(),
            "case {target}: the sibling resource must survive, got {survivor:?}"
        );

        // A second delete of the same name is refused: the entry is really gone.
        let (repeat_status, repeat) = emulator.call(target, &body).await;
        assert_eq!(
            repeat_status, 400,
            "case {target}: deleting twice must be refused, got {repeat_status} {repeat}"
        );
    }

    let listed = emulator.ok("ListModels", &json!({})).await;
    let names: Vec<String> = listed
        .get("Models")
        .and_then(Value::as_array)
        .expect("Models")
        .iter()
        .map(|model| str_field(model, "ModelName"))
        .collect();
    assert_eq!(
        names,
        vec!["keep-model".to_string()],
        "a deleted model must not be listed, got {listed:?}"
    );
}

/// Create four models, spaced so `CreationTime` ordering is unambiguous.
async fn seed_models(emulator: &Emulator) -> Vec<(String, f64)> {
    let mut seeded = Vec::new();
    for name in ["alpha-one", "beta-two", "demo-three", "demo-four"] {
        emulator
            .ok(
                "CreateModel",
                &json!({ "ModelName": name, "PrimaryContainer": primary_container("demo:1") }),
            )
            .await;
        let described = emulator
            .ok("DescribeModel", &json!({ "ModelName": name }))
            .await;
        seeded.push((
            name.to_string(),
            described
                .get("CreationTime")
                .and_then(Value::as_f64)
                .expect("CreationTime"),
        ));
        tokio::time::sleep(CREATE_SPACING).await;
    }
    seeded
}

fn listed_names(body: &Value) -> Vec<String> {
    body.get("Models")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a Models list in {body}"))
        .iter()
        .map(|model| str_field(model, "ModelName"))
        .collect()
}

/// RULE: `ListModels` returns the stored models filtered by `NameContains`, ordered by
/// `SortBy`/`SortOrder` (defaulting to `CreationTime` / `Descending`, as both members'
/// model docs state), and each entry carries the stored name, ARN and creation time.
#[tokio::test]
async fn list_models_honours_every_documented_sort_and_filter() {
    let emulator = Emulator::new();
    let seeded = seed_models(&emulator).await;

    // Rows differ along sort field, sort order, and the filter's selectivity.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "documented-defaults",
            json!({}),
            vec!["demo-four", "demo-three", "beta-two", "alpha-one"],
        ),
        (
            "name-ascending",
            json!({ "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["alpha-one", "beta-two", "demo-four", "demo-three"],
        ),
        (
            "name-descending",
            json!({ "SortBy": "Name", "SortOrder": "Descending" }),
            vec!["demo-three", "demo-four", "beta-two", "alpha-one"],
        ),
        (
            "creation-time-ascending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
            vec!["alpha-one", "beta-two", "demo-three", "demo-four"],
        ),
        (
            "creation-time-descending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Descending" }),
            vec!["demo-four", "demo-three", "beta-two", "alpha-one"],
        ),
        (
            "name-contains-two-matches",
            json!({ "NameContains": "demo", "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["demo-four", "demo-three"],
        ),
        (
            "name-contains-one-match",
            json!({ "NameContains": "alpha", "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["alpha-one"],
        ),
        (
            "name-contains-no-match",
            json!({ "NameContains": "nothing-here" }),
            vec![],
        ),
    ];

    for (case, body, expected) in cases {
        let listed = emulator.ok("ListModels", &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: ListModels order, got {listed:?}"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {case}: a complete page must carry no NextToken, got {listed:?}"
        );
    }

    // Each summary replays the stored name, ARN and creation time.
    let listed = emulator
        .ok(
            "ListModels",
            &json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
        )
        .await;
    let models = listed.get("Models").and_then(Value::as_array).expect("Models");
    for (index, (name, created_at)) in seeded.iter().enumerate() {
        let summary = &models[index];
        assert_eq!(
            str_field(summary, "ModelArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model/{name}"),
            "case {name}: ModelArn in the summary, got {summary:?}"
        );
        assert_eq!(
            summary.get("CreationTime").and_then(Value::as_f64),
            Some(*created_at),
            "case {name}: CreationTime in the summary must be the stored one"
        );
    }
}

/// RULE: a `CreationTimeBefore` / `CreationTimeAfter` bound selects exactly the models on
/// that side of it.
#[tokio::test]
async fn list_models_filters_on_the_creation_time_bounds() {
    let emulator = Emulator::new();
    let seeded = seed_models(&emulator).await;
    // A bound strictly between the second and third model, so the split is unambiguous.
    let split = (seeded[1].1 + seeded[2].1) / 2.0;

    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "before-the-split",
            json!({ "CreationTimeBefore": split, "SortBy": "CreationTime", "SortOrder": "Ascending" }),
            vec!["alpha-one", "beta-two"],
        ),
        (
            "after-the-split",
            json!({ "CreationTimeAfter": split, "SortBy": "CreationTime", "SortOrder": "Ascending" }),
            vec!["demo-three", "demo-four"],
        ),
        (
            "both-bounds-around-one-model",
            json!({
                "CreationTimeAfter": split,
                "CreationTimeBefore": (seeded[2].1 + seeded[3].1) / 2.0,
                "SortBy": "CreationTime",
                "SortOrder": "Ascending",
            }),
            vec!["demo-three"],
        ),
        (
            "bounds-that-exclude-everything",
            json!({ "CreationTimeAfter": seeded[3].1 + 3600.0 }),
            vec![],
        ),
        (
            "bound-combined-with-a-name-filter",
            json!({ "CreationTimeAfter": split, "NameContains": "demo", "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["demo-four", "demo-three"],
        ),
    ];

    for (case, body, expected) in cases {
        let listed = emulator.ok("ListModels", &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: ListModels selection, got {listed:?}"
        );
    }
}

/// RULE: paging with `MaxResults` walks the whole ordered result exactly once — each page
/// holds at most `MaxResults` entries, a `NextToken` is present only while more remain,
/// and feeding it back resumes immediately after the previous page.
#[tokio::test]
async fn paging_with_max_results_walks_the_whole_result_exactly_once() {
    let emulator = Emulator::new();
    seed_models(&emulator).await;
    let ordered = vec!["alpha-one", "beta-two", "demo-four", "demo-three"];

    // Rows: every page size from one entry up to past the end of the result set.
    // `MaxResults` is `range 1..100` (smithy_query operation=ListModels), so 1 and 100
    // are the published bounds.
    for page_size in [1_i64, 2, 3, 4, 5, 100] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut body = json!({
                "SortBy": "Name",
                "SortOrder": "Ascending",
                "MaxResults": page_size,
            });
            if let Some(ref value) = token {
                body["NextToken"] = Value::String(value.clone());
            }
            let listed = emulator.ok("ListModels", &body).await;
            let names = listed_names(&listed);
            assert!(
                names.len() as i64 <= page_size,
                "case {case}: a page must hold at most {page_size} entries, got {names:?}"
            );
            seen.extend(names);
            pages += 1;
            assert!(
                pages <= ordered.len() + 1,
                "case {case}: paging did not terminate, saw {seen:?}"
            );
            match listed.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        assert_eq!(
            seen, ordered,
            "case {case}: paging must yield the whole ordered result exactly once"
        );
        let expected_pages = (ordered.len() as i64 + page_size - 1) / page_size;
        assert_eq!(
            pages as i64, expected_pages,
            "case {case}: page count, saw {seen:?}"
        );
    }

    // A token that names no boundary of this result set is refused rather than silently
    // restarting the listing.
    let (status, parsed) = emulator
        .call("ListModels", &json!({ "NextToken": "not-a-token" }))
        .await;
    assert_eq!(
        status, 400,
        "case bogus-NextToken: expected HTTP 400, got {status} {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ValidationException",
        "case bogus-NextToken: error code, got {parsed:?}"
    );
}

/// RULE: the page is capped server-side, so no single `ListModels` call serialises the
/// whole store however large `MaxResults` is or whether the caller sends it at all — and
/// the resume token still walks the full set.
#[tokio::test]
async fn the_page_is_capped_server_side_even_when_the_caller_asks_for_more() {
    // `state::MAX_PAGE_SIZE` is this service's own ceiling; the store is seeded past it
    // so a capped page and an uncapped one are distinguishable.
    const CEILING: usize = 50;
    const STORED: usize = CEILING + 7;

    let emulator = Emulator::new();
    for index in 0..STORED {
        emulator
            .ok(
                "CreateModel",
                &json!({
                    "ModelName": format!("m{index:03}"),
                    "PrimaryContainer": primary_container("demo:1"),
                }),
            )
            .await;
    }

    // Rows: no `MaxResults` at all, the modelled maximum (above the ceiling), and a
    // value below the ceiling, which still wins because it is the smaller cap.
    let cases: Vec<(&str, Value, usize)> = vec![
        ("max-results-absent", json!({}), CEILING),
        ("max-results-at-the-modelled-maximum", json!({ "MaxResults": 100 }), CEILING),
        ("max-results-below-the-ceiling", json!({ "MaxResults": 10 }), 10),
    ];

    for (case, extra, expected_page) in cases {
        let mut body = json!({ "SortBy": "Name", "SortOrder": "Ascending" });
        for (key, value) in extra.as_object().expect("a JSON object of extra members") {
            body[key] = value.clone();
        }
        let listed = emulator.ok("ListModels", &body).await;
        assert_eq!(
            listed_names(&listed).len(),
            expected_page,
            "case {case}: page size, got {listed:?}"
        );
        assert!(
            listed.get("NextToken").is_some(),
            "case {case}: more models remain, so a NextToken is required, got {listed:?}"
        );
    }

    // And the capped pages still walk the whole store exactly once.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({ "SortBy": "Name", "SortOrder": "Ascending" });
        if let Some(ref value) = token {
            body["NextToken"] = Value::String(value.clone());
        }
        let listed = emulator.ok("ListModels", &body).await;
        let names = listed_names(&listed);
        assert!(
            names.len() <= CEILING,
            "case capped-walk: a page must never exceed the ceiling, got {} entries",
            names.len()
        );
        seen.extend(names);
        assert!(
            seen.len() <= STORED,
            "case capped-walk: paging did not terminate, saw {} entries",
            seen.len()
        );
        match listed.get("NextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    let expected: Vec<String> = (0..STORED).map(|index| format!("m{index:03}")).collect();
    assert_eq!(
        seen, expected,
        "case capped-walk: the capped pages must cover the whole store exactly once"
    );
}

/// RULE: an endpoint's `DataCaptureConfig` summary is derived from the configuration's
/// stored capture settings — `CaptureStatus` follows `EnableCapture` (which the model doc
/// says "defaults to enabled"), the sampling percentage and destination are replayed, and
/// an absent `KmsKeyId` reports the empty string the required summary member allows.
#[tokio::test]
async fn the_endpoint_data_capture_summary_is_derived_from_the_stored_capture_config() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateModel",
            &json!({ "ModelName": "dc-model", "PrimaryContainer": primary_container("demo:1") }),
        )
        .await;

    // Rows differ along: capture enabled explicitly, disabled explicitly, left to the
    // documented default, and with vs without a KMS key.
    let cases: Vec<(&str, Value, bool, &str, i64, &str)> = vec![
        (
            "enabled-with-kms",
            json!({
                "EnableCapture": true,
                "InitialSamplingPercentage": 25,
                "DestinationS3Uri": "s3://bucket/one",
                "CaptureOptions": [{ "CaptureMode": "Input" }],
                "KmsKeyId": "alias/capture",
            }),
            true,
            "Started",
            25,
            "alias/capture",
        ),
        (
            "disabled-without-kms",
            json!({
                "EnableCapture": false,
                "InitialSamplingPercentage": 10,
                "DestinationS3Uri": "s3://bucket/two",
                "CaptureOptions": [{ "CaptureMode": "Output" }],
            }),
            false,
            "Stopped",
            10,
            "",
        ),
        (
            "enable-capture-left-to-the-documented-default",
            json!({
                "InitialSamplingPercentage": 100,
                "DestinationS3Uri": "s3://bucket/three",
                "CaptureOptions": [
                    { "CaptureMode": "Input" },
                    { "CaptureMode": "Output" },
                ],
            }),
            true,
            "Started",
            100,
            "",
        ),
    ];

    for (index, (case, capture, enabled, status, sampling, key)) in cases.into_iter().enumerate() {
        let config_name = format!("dc-cfg{index}");
        let endpoint_name = format!("dc-ep{index}");
        let mut config = one_variant_config(&config_name, "dc-model");
        config["DataCaptureConfig"] = capture;
        emulator.ok("CreateEndpointConfig", &config).await;
        emulator
            .ok(
                "CreateEndpoint",
                &json!({ "EndpointName": endpoint_name, "EndpointConfigName": config_name }),
            )
            .await;

        let described = emulator
            .ok("DescribeEndpoint", &json!({ "EndpointName": endpoint_name }))
            .await;
        let summary = described
            .get("DataCaptureConfig")
            .unwrap_or_else(|| panic!("case {case}: DescribeEndpoint must report the summary"));
        assert_eq!(
            summary.get("EnableCapture").and_then(Value::as_bool),
            Some(enabled),
            "case {case}: EnableCapture, got {summary:?}"
        );
        assert_eq!(
            str_field(summary, "CaptureStatus"),
            status,
            "case {case}: CaptureStatus, got {summary:?}"
        );
        assert_eq!(
            summary
                .get("CurrentSamplingPercentage")
                .and_then(Value::as_i64),
            Some(sampling),
            "case {case}: CurrentSamplingPercentage, got {summary:?}"
        );
        assert_eq!(
            str_field(summary, "KmsKeyId"),
            key,
            "case {case}: KmsKeyId, got {summary:?}"
        );
    }

    // An endpoint whose configuration carried no capture settings omits the member.
    emulator
        .ok(
            "CreateEndpointConfig",
            &one_variant_config("dc-none-cfg", "dc-model"),
        )
        .await;
    emulator
        .ok(
            "CreateEndpoint",
            &json!({ "EndpointName": "dc-none-ep", "EndpointConfigName": "dc-none-cfg" }),
        )
        .await;
    let described = emulator
        .ok("DescribeEndpoint", &json!({ "EndpointName": "dc-none-ep" }))
        .await;
    assert!(
        described.get("DataCaptureConfig").is_none(),
        "case no-capture-config: the member must be omitted, got {described:?}"
    );
}

/// RULE: each store holds at most its quota of records; the create past it is refused
/// with the modelled `ResourceLimitExceeded` and stores nothing, so no caller can grow
/// the emulator's state without end by varying the resource name.
#[tokio::test]
async fn a_create_past_a_store_quota_is_refused_with_the_modelled_limit_failure() {
    // `state::MODEL_QUOTA` / `ENDPOINT_CONFIG_QUOTA` / `ENDPOINT_QUOTA`, all 1_000.
    const QUOTA: usize = 1_000;

    // Rows: all three bounded stores, each filled to its quota and then pushed one past
    // it, with the reader that proves the refused record was not stored.
    for store in ["models", "endpoint-configs", "endpoints"] {
        let emulator = Emulator::new();
        emulator
            .ok(
                "CreateModel",
                &json!({ "ModelName": "quota-model", "PrimaryContainer": primary_container("demo:1") }),
            )
            .await;
        emulator
            .ok(
                "CreateEndpointConfig",
                &one_variant_config("quota-cfg", "quota-model"),
            )
            .await;

        // Fill the store under test up to its quota (one record of it already exists).
        let (target, name_member, reader, fill_body): (&str, &str, &str, fn(&str) -> Value) =
            match store {
                "models" => (
                    "CreateModel",
                    "ModelName",
                    "DescribeModel",
                    |name| json!({ "ModelName": name, "PrimaryContainer": { "Image": "demo:1" } }),
                ),
                "endpoint-configs" => (
                    "CreateEndpointConfig",
                    "EndpointConfigName",
                    "DescribeEndpointConfig",
                    |name| one_variant_config(name, "quota-model"),
                ),
                _ => (
                    "CreateEndpoint",
                    "EndpointName",
                    "DescribeEndpoint",
                    |name| json!({ "EndpointName": name, "EndpointConfigName": "quota-cfg" }),
                ),
            };
        let already_stored = usize::from(store != "endpoints");
        for index in already_stored..QUOTA {
            emulator.ok(target, &fill_body(&format!("q{index:04}"))).await;
        }

        let overflow = format!("q{QUOTA:04}");
        let (status, parsed) = emulator.call(target, &fill_body(&overflow)).await;
        assert_eq!(
            status, 400,
            "case {store}: a create past the quota must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceLimitExceeded",
            "case {store}: the refusal must use the operation's modelled limit error, \
             got {parsed:?}"
        );

        // Nothing was stored for the refused request.
        let (read_status, read_body) = emulator
            .call(reader, &json!({ name_member: overflow.clone() }))
            .await;
        assert_eq!(
            read_status, 400,
            "case {store}: the refused record must not be stored, got {read_status} {read_body}"
        );

        // And a delete frees a slot, so the same create then succeeds: the bound is a
        // live count, not a one-way counter.
        let freed = format!("q{:04}", QUOTA - 1);
        let delete_target = match store {
            "models" => "DeleteModel",
            "endpoint-configs" => continue, // no DeleteEndpointConfig in this slice
            _ => "DeleteEndpoint",
        };
        emulator
            .ok(delete_target, &json!({ name_member: freed }))
            .await;
        emulator.ok(target, &fill_body(&overflow)).await;
    }
}

/// RULE: a request outside a member's modelled constraint is refused at the boundary with
/// HTTP 400, before any state is written.
#[tokio::test]
async fn requests_outside_the_modelled_constraints_are_refused() {
    let emulator = Emulator::new();

    // Rows: one past each published bound the model states for these operations —
    // `ModelName` len 0..63, `ProductionVariants` len 1..10, `MaxResults` range 1..100.
    let too_long = "a".repeat(64);
    let eleven_variants: Vec<Value> = (0..11)
        .map(|index| json!({ "VariantName": format!("v{index}") }))
        .collect();
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "model-name-past-63",
            "CreateModel",
            json!({ "ModelName": too_long }),
        ),
        (
            "model-name-with-an-illegal-character",
            "CreateModel",
            json!({ "ModelName": "not a model name" }),
        ),
        (
            "zero-production-variants",
            "CreateEndpointConfig",
            json!({ "EndpointConfigName": "empty", "ProductionVariants": [] }),
        ),
        (
            "eleven-production-variants",
            "CreateEndpointConfig",
            json!({ "EndpointConfigName": "eleven", "ProductionVariants": eleven_variants }),
        ),
        (
            "max-results-below-1",
            "ListModels",
            json!({ "MaxResults": 0 }),
        ),
        (
            "max-results-above-100",
            "ListModels",
            json!({ "MaxResults": 101 }),
        ),
    ];

    for (case, target, body) in cases {
        let (status, parsed) = emulator.call(target, &body).await;
        assert_eq!(
            status, 400,
            "case {case}: {target} must be refused, got {status} {parsed}"
        );
    }

    // Nothing above reached state: the store is still empty.
    let listed = emulator.ok("ListModels", &json!({})).await;
    assert_eq!(
        listed_names(&listed),
        Vec::<String>::new(),
        "a refused request must write nothing, got {listed:?}"
    );
}
