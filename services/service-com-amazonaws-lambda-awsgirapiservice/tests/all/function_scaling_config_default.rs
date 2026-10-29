// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `GetFunctionScalingConfig` reports for a qualifier no
//! `PutFunctionScalingConfig` call has ever set bounds on.
//!
//! The rule under test: scaling a Lambda Managed Instances function is the SERVICE's job
//! from the moment the function exists, so every qualifier of a function whose
//! configuration names a capacity provider reports a requested scaling configuration
//! straight away — the service-managed floor of execution environments, with no maximum,
//! because nothing caps the function until a caller asks for a cap. No qualifier reports an
//! APPLIED configuration until one is in effect, a `PutFunctionScalingConfig` call on a
//! qualifier REPLACES what the service manages for that qualifier alone, and reading
//! changes nothing a later caller sees. (A function that runs on no capacity provider is
//! outside the surface these operations serve at all; its refusal is covered by
//! `function_scaling_config_capacity_provider.rs`.)
//!
//! Every row drives real HTTP requests through the router, and the configurations that
//! replace a default are read back through `GetFunctionScalingConfig`, so a value that was
//! answered but never stored fails here. The rows differ along what the rule generalises
//! over: the kind of qualifier (the published view of the unpublished configuration,
//! written raw and percent-encoded, and published version numbers — single- and
//! multi-digit), how many versions the function has published, and the documented
//! `FunctionName` form the requests address the function by.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where `CreateCapacityProvider` is bound, from
/// `smithy_query(query="operation", operation="CreateCapacityProvider")`.
const CAPACITY_PROVIDERS_URI: &str = "/2025-11-30/capacity-providers";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in
/// `us-east-1`, which is what routes a request here and what region the ARNs carry.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// A well-formed capacity-provider operator role ARN.
const OPERATOR_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-capacity";

/// The capacity provider every Managed Instances function here runs on.
const FLEET: &str = "fleet-managed";

/// The published view of the unpublished configuration, one of the two forms the model's
/// `PublishedFunctionQualifier` admits
/// (`smithy_query(query="shape", shape="PublishedFunctionQualifier")`:
/// `^(\$LATEST\.PUBLISHED|[0-9]+)$`).
const LATEST_PUBLISHED: &str = "$LATEST.PUBLISHED";

/// The minimum number of execution environments the service keeps a Managed Instances
/// function's qualifier at while nothing else has been asked for. Written as the number
/// itself rather than as a crate constant, so a changed floor fails here. It sits inside
/// the published range of both `FunctionScalingConfig` members
/// (`smithy_query(query="shape", shape="FunctionScalingConfig")`: `range 0..15000`).
const SERVICE_MANAGED_MINIMUM: i64 = 3;

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The value at `pointer`, or `Value::Null` when the body carries none there.
    fn at(&self, pointer: &str) -> Value {
        self.json().pointer(pointer).cloned().unwrap_or(Value::Null)
    }

    /// The string at `pointer`, when the body carries one there.
    fn string(&self, pointer: &str) -> Option<String> {
        self.at(pointer).as_str().map(str::to_string)
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// Where both function-scaling operations of one function are bound.
fn scaling_uri(reference: &str, qualifier: &str) -> String {
    format!("/2025-11-30/functions/{reference}/function-scaling-config?Qualifier={qualifier}")
}

/// `GetFunctionScalingConfig`
/// (`GET /2025-11-30/functions/{FunctionName}/function-scaling-config?Qualifier=`).
async fn get_scaling(router: &MultiServiceRouter, reference: &str, qualifier: &str) -> Answer {
    call(
        router,
        "GET",
        &scaling_uri(reference, qualifier),
        &Value::Null,
    )
    .await
}

/// `PutFunctionScalingConfig` on the same resource, with the bounds `bounds` states.
async fn put_scaling(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: &str,
    bounds: (i64, i64),
) -> Answer {
    call(
        router,
        "PUT",
        &scaling_uri(reference, qualifier),
        &json!({
            "FunctionScalingConfig": {
                "MinExecutionEnvironments": bounds.0,
                "MaxExecutionEnvironments": bounds.1,
            },
        }),
    )
    .await
}

/// The ARN a capacity provider of `name` carries, as the model's `CapacityProviderArn`
/// pattern spells it.
fn provider_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider:{name}")
}

/// The ARN a configuration is reported against: the model's `FunctionArn` admits
/// `$LATEST` or a published version as an ARN's qualifier but no `.`-bearing label, so the
/// published view of the unpublished configuration is the function's unqualified ARN and a
/// published version its own qualified one.
fn expected_arn(function: &str, qualifier: &str) -> String {
    if qualifier == LATEST_PUBLISHED {
        return format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{function}");
    }
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{function}:{qualifier}")
}

/// `CreateCapacityProvider` with the smallest request the model admits.
async fn create_provider(router: &MultiServiceRouter, name: &str) {
    let answer = call(
        router,
        "POST",
        CAPACITY_PROVIDERS_URI,
        &json!({
            "CapacityProviderName": name,
            "VpcConfig": { "SubnetIds": ["subnet-10000000"], "SecurityGroupIds": [] },
            "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OPERATOR_ROLE },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        202,
        "provider {name:?}: CreateCapacityProvider must answer 202, got {} {}",
        answer.status,
        answer.text()
    );
}

/// Create a Managed Instances function with `versions` published versions, running on
/// `provider`, and bring it out of the `Pending` state creation leaves it in.
async fn create_function_with_versions(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
    provider: &str,
    versions: u32,
) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let body = json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Publish": versions >= 1,
        "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        "CapacityProviderConfig": {
            "LambdaManagedInstancesCapacityProviderConfig": {
                "CapacityProviderArn": provider_arn(provider),
            },
        },
    });
    let created = call(router, "POST", FUNCTIONS_URI, &body).await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        created.status,
        created.text()
    );
    for version in 2..=versions {
        let published = call(
            router,
            "POST",
            &format!("{FUNCTIONS_URI}/{name}/versions"),
            &json!({ "Description": format!("snapshot {version}") }),
        )
        .await;
        assert_eq!(
            published.status,
            201,
            "PublishVersion {version} of {name:?} must succeed, got {} {}",
            published.status,
            published.text()
        );
    }
    make_activation_due(engine);
}

/// Move every stored function's activation instant into the past, so the next read
/// materialises the `Pending` -> `Active` transition the clock owes. This is what waiting
/// would do, without waiting.
fn make_activation_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
}

/// Move every stored scaling configuration's application instant into the past, so the
/// next read materialises the requested -> applied move the clock owes.
fn make_scaling_applied(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        for config in function.function_scaling_configs.values_mut() {
            if config.applied_due.is_some() {
                config.applied_due = Some(past);
            }
        }
    }
}

/// One row of the default-reporting table.
struct Case {
    name: &'static str,
    /// The capacity provider the function's configuration names.
    provider: &'static str,
    /// How many versions the function publishes.
    versions: u32,
    /// The qualifier every call under test addresses, as the request writes it.
    qualifier: &'static str,
    /// The qualifier label the answer is reported against, once decoded.
    decoded_qualifier: &'static str,
    /// The `FunctionName` form, built from the function name; `None` means the bare name.
    reference: Option<fn(&str) -> String>,
}

/// The rows. Between them they cover the published view of the unpublished configuration
/// (raw and percent-encoded) and single- and multi-digit published version numbers as the
/// qualifier, functions with no published version and with many, and each documented
/// `FunctionName` form.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "managed instances, published view of latest, bare name",
            provider: FLEET,
            versions: 0,
            qualifier: LATEST_PUBLISHED,
            decoded_qualifier: LATEST_PUBLISHED,
            reference: None,
        },
        Case {
            name: "managed instances, published view of latest percent-encoded, full ARN",
            provider: FLEET,
            versions: 0,
            qualifier: "%24LATEST.PUBLISHED",
            decoded_qualifier: LATEST_PUBLISHED,
            reference: Some(|name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")),
        },
        Case {
            name: "managed instances, version 1, partial ARN",
            provider: FLEET,
            versions: 1,
            qualifier: "1",
            decoded_qualifier: "1",
            reference: Some(|name| format!("{ACCOUNT}:function:{name}")),
        },
        Case {
            name: "managed instances, version 11, name with the resource-type prefix",
            provider: FLEET,
            versions: 12,
            qualifier: "11",
            decoded_qualifier: "11",
            reference: Some(|name| format!("function:{name}")),
        },
    ]
}

/// The requested configuration every row expects before any `PutFunctionScalingConfig`
/// call: the service-managed floor, with no maximum.
fn expected_default() -> Value {
    json!({ "MinExecutionEnvironments": SERVICE_MANAGED_MINIMUM })
}

/// Every row: a qualifier nobody has configured reports what the service scales it by, and
/// reports no applied configuration, and reading it twice answers the same thing.
#[tokio::test]
async fn unconfigured_qualifier_reports_the_service_managed_configuration() {
    for case in cases() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let function = "managed-function";
        create_provider(&router, FLEET).await;
        create_function_with_versions(&router, &engine, function, case.provider, case.versions)
            .await;
        let reference = case
            .reference
            .map_or_else(|| function.to_string(), |build| build(function));

        let answer = get_scaling(&router, &reference, case.qualifier).await;
        assert_eq!(
            answer.status,
            200,
            "case {:?}: GetFunctionScalingConfig must answer 200, got {} {}",
            case.name,
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.at("/RequestedFunctionScalingConfig"),
            expected_default(),
            "case {:?}: expected the requested configuration {}, got {}",
            case.name,
            expected_default(),
            answer.text()
        );
        assert_eq!(
            answer.at("/AppliedFunctionScalingConfig"),
            Value::Null,
            "case {:?}: nothing is in effect before a configuration is applied, got {}",
            case.name,
            answer.text()
        );
        assert_eq!(
            answer.string("/FunctionArn"),
            Some(expected_arn(function, case.decoded_qualifier)),
            "case {:?}: the read must report the qualifier's ARN, got {}",
            case.name,
            answer.text()
        );

        // A read reports; it stores nothing, so the next caller sees exactly the same
        // answer.
        let again = get_scaling(&router, &reference, case.qualifier).await;
        assert_eq!(
            again.json(),
            answer.json(),
            "case {:?}: a second read must answer the same thing, got {} then {}",
            case.name,
            answer.text(),
            again.text()
        );
    }
}

/// Every row: a `PutFunctionScalingConfig` call replaces the service-managed configuration
/// on the qualifier it addressed and on no other, and the bounds it asked for are in effect
/// once the clock makes that due.
#[tokio::test]
async fn an_explicit_configuration_replaces_the_service_managed_one() {
    let bounds = (1_i64, 9_i64);
    let replacement = json!({
        "MinExecutionEnvironments": bounds.0,
        "MaxExecutionEnvironments": bounds.1,
    });
    for case in cases() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let function = "managed-function";
        create_provider(&router, FLEET).await;
        // One published version beside the row's own qualifier, so a configuration set on
        // one qualifier is shown not to reach another.
        let versions = case.versions.max(1);
        create_function_with_versions(&router, &engine, function, case.provider, versions).await;
        let reference = case
            .reference
            .map_or_else(|| function.to_string(), |build| build(function));

        let stored = put_scaling(&router, &reference, case.qualifier, bounds).await;
        assert_eq!(
            stored.status,
            202,
            "case {:?}: PutFunctionScalingConfig {bounds:?} must answer 202, got {} {}",
            case.name,
            stored.status,
            stored.text()
        );

        let requested = get_scaling(&router, &reference, case.qualifier).await;
        assert_eq!(
            requested.at("/RequestedFunctionScalingConfig"),
            replacement,
            "case {:?}: expected the stored bounds {bounds:?} reported as requested, got {}",
            case.name,
            requested.text()
        );
        assert_eq!(
            requested.at("/AppliedFunctionScalingConfig"),
            Value::Null,
            "case {:?}: bounds are not in effect before the clock applies them, got {}",
            case.name,
            requested.text()
        );

        // Another qualifier of the same function is untouched: the service still manages
        // it.
        let other_qualifier = if case.decoded_qualifier == "1" {
            LATEST_PUBLISHED
        } else {
            "1"
        };
        let other = get_scaling(&router, &reference, other_qualifier).await;
        assert_eq!(
            other.at("/RequestedFunctionScalingConfig"),
            json!({ "MinExecutionEnvironments": SERVICE_MANAGED_MINIMUM }),
            "case {:?}: qualifier {other_qualifier:?} must still report the service-managed \
             configuration, got {}",
            case.name,
            other.text()
        );

        make_scaling_applied(&engine);
        let applied = get_scaling(&router, &reference, case.qualifier).await;
        assert_eq!(
            applied.at("/AppliedFunctionScalingConfig"),
            replacement,
            "case {:?}: expected the bounds {bounds:?} in effect once applied, got {}",
            case.name,
            applied.text()
        );
    }
}
