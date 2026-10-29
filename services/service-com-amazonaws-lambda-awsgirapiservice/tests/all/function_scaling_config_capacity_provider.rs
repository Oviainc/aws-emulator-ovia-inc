// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for which functions the function-scaling operations serve at all:
//! `GetFunctionScalingConfig`, which "Retrieves the scaling configuration for a Lambda
//! Managed Instances function", and `PutFunctionScalingConfig`, which "Sets" it.
//!
//! The rule under test: both operations serve a Lambda Managed Instances function — one
//! whose configuration names a capacity provider — so once the function and the qualifier
//! the request named have resolved, a qualifier whose own configuration names no capacity
//! provider is REFUSED `AccessDeniedException` (HTTP 403) rather than served, and the
//! refusal stores nothing. The refusal comes after resolution, so it never masks the
//! modelled 404 a name or qualifier that names no resource gets, and a qualifier that does
//! name a capacity provider is served.
//!
//! Every row drives real HTTP requests through the router. The rows differ along what the
//! rule generalises over: the operation (the read and the write), whether the qualifier's
//! configuration names a capacity provider, the kind of qualifier (the published view of
//! the unpublished configuration, written raw and percent-encoded, and single- and
//! multi-digit published version numbers), how many versions the function has published,
//! and the documented `FunctionName` form the request addresses the function by. One row
//! pair separates the two: a version published BEFORE a capacity provider was added to the
//! function froze a configuration that names none, so it is refused while the published
//! view of the updated configuration is served.

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

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`,
/// which is what routes a request here and what region the ARNs carry. Nothing in it is
/// verified: the harness turns request authentication off.
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

/// The wire error code a function that runs on no capacity provider is refused with. It is
/// in no operation's modelled error list: the service answers it for a call whose resource
/// is outside the surface the operation serves.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// The status that code carries.
const FORBIDDEN: u16 = 403;

/// The service's wording for that refusal: a fixed sentence naming nothing of the request,
/// because the caller already names the function it asked about.
const NO_CAPACITY_PROVIDER_MESSAGE: &str =
    "The function provided by the arn does not contain a capacity provider configuration";

/// The sentence these operations refuse a name or qualifier that names no resource with:
/// the service's canonical `ResourceNotFoundException` wording, which likewise names
/// nothing of the request.
const RESOURCE_NOT_FOUND_MESSAGE: &str = "The resource you requested does not exist.";

/// The minimum number of execution environments the service itself keeps a Managed
/// Instances function's qualifier at while no call has asked for anything else. Written as
/// the number itself rather than as a crate constant, so a changed floor fails here.
const SERVICE_MANAGED_MINIMUM: i64 = 3;

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
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

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it.
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
        headers: parts.headers,
        body,
    }
}

/// Where both function-scaling operations of one qualifier are bound.
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

/// `PutFunctionScalingConfig` on the same resource, asking for `bounds`.
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

/// The `CapacityProviderConfig` that makes a function a Managed Instances one: the model's
/// `CapacityProviderConfig` carries exactly one
/// `LambdaManagedInstancesCapacityProviderConfig`, whose `CapacityProviderArn` is required.
fn capacity_provider_config(name: &str) -> Value {
    json!({
        "LambdaManagedInstancesCapacityProviderConfig": {
            "CapacityProviderArn": provider_arn(name),
        },
    })
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

/// Create a function with `versions` published versions, running on `provider` when one is
/// named, and bring it out of the `Pending` state creation leaves it in.
async fn create_function_with_versions(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
    provider: Option<&str>,
    versions: u32,
) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let mut body = json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Publish": versions >= 1,
        "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
    });
    if let Some(provider) = provider {
        body["CapacityProviderConfig"] = capacity_provider_config(provider);
    }
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
/// materialises the transition the clock owes. This is what waiting would do, without
/// waiting.
fn make_activation_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
}

/// One row: the qualifier a function-scaling call addresses, whether that qualifier's
/// configuration names a capacity provider, and the `FunctionName` form the call addresses
/// the function by.
struct Case {
    name: &'static str,
    /// The capacity provider the function's configuration names; `None` for a function
    /// that runs on no provider.
    provider: Option<&'static str>,
    /// How many versions the function publishes.
    versions: u32,
    /// The qualifier every call under test addresses, as the request writes it.
    qualifier: &'static str,
    /// The qualifier label an answer is reported against, once decoded.
    decoded_qualifier: &'static str,
    /// The `FunctionName` form, built from the function name; `None` means the bare name.
    reference: Option<fn(&str) -> String>,
}

/// The rows. Between them they cover a function that names a capacity provider and one that
/// names none, the published view of the unpublished configuration (raw and
/// percent-encoded) and single- and multi-digit published version numbers as the qualifier,
/// functions with no published version and with many, and each documented `FunctionName`
/// form.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "no capacity provider, published view of latest, bare name",
            provider: None,
            versions: 0,
            qualifier: LATEST_PUBLISHED,
            decoded_qualifier: LATEST_PUBLISHED,
            reference: None,
        },
        Case {
            name: "no capacity provider, published view of latest percent-encoded, full ARN",
            provider: None,
            versions: 0,
            qualifier: "%24LATEST.PUBLISHED",
            decoded_qualifier: LATEST_PUBLISHED,
            reference: Some(|name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")),
        },
        Case {
            name: "no capacity provider, version 1, partial ARN",
            provider: None,
            versions: 1,
            qualifier: "1",
            decoded_qualifier: "1",
            reference: Some(|name| format!("{ACCOUNT}:function:{name}")),
        },
        Case {
            name: "no capacity provider, version 11, name with the resource-type prefix",
            provider: None,
            versions: 12,
            qualifier: "11",
            decoded_qualifier: "11",
            reference: Some(|name| format!("function:{name}")),
        },
        Case {
            name: "managed instances, published view of latest, bare name",
            provider: Some(FLEET),
            versions: 0,
            qualifier: LATEST_PUBLISHED,
            decoded_qualifier: LATEST_PUBLISHED,
            reference: None,
        },
        Case {
            name: "managed instances, version 11, full ARN",
            provider: Some(FLEET),
            versions: 12,
            qualifier: "11",
            decoded_qualifier: "11",
            reference: Some(|name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")),
        },
    ]
}

/// The ARN a configuration is reported against: the model's `FunctionArn` admits `$LATEST`
/// or a published version as an ARN's qualifier but no `.`-bearing label, so the published
/// view of the unpublished configuration is the function's unqualified ARN and a published
/// version its own qualified one.
fn expected_arn(function: &str, qualifier: &str) -> String {
    if qualifier == LATEST_PUBLISHED {
        return format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{function}");
    }
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{function}:{qualifier}")
}

/// Assert that `answer` is the refusal a qualifier running on no capacity provider gets.
fn assert_refused(answer: &Answer, case: &str, operation: &str) {
    assert_eq!(
        answer.status,
        FORBIDDEN,
        "case {case:?}: {operation} must be refused {FORBIDDEN}, got {} {}",
        answer.status,
        answer.text()
    );
    assert!(
        answer.error_code().contains(ACCESS_DENIED),
        "case {case:?}: {operation} must be refused {ACCESS_DENIED}, got {} {}",
        answer.error_code(),
        answer.text()
    );
    assert_eq!(
        answer.string("/Message").as_deref(),
        Some(NO_CAPACITY_PROVIDER_MESSAGE),
        "case {case:?}: {operation} must carry the service's own wording, got {}",
        answer.text()
    );
}

/// Every row: a qualifier whose configuration names no capacity provider is refused by both
/// operations, and one that names a provider is served by both.
#[tokio::test]
async fn a_qualifier_on_no_capacity_provider_is_refused_by_both_operations() {
    for case in cases() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let function = "scaled-function";
        create_provider(&router, FLEET).await;
        create_function_with_versions(&router, &engine, function, case.provider, case.versions)
            .await;
        let reference = case
            .reference
            .map_or_else(|| function.to_string(), |build| build(function));

        let read = get_scaling(&router, &reference, case.qualifier).await;
        let written = put_scaling(&router, &reference, case.qualifier, (1, 9)).await;
        if case.provider.is_none() {
            assert_refused(&read, case.name, "GetFunctionScalingConfig");
            assert_refused(&written, case.name, "PutFunctionScalingConfig");
            continue;
        }

        assert_eq!(
            read.status,
            200,
            "case {:?}: GetFunctionScalingConfig must answer 200, got {} {}",
            case.name,
            read.status,
            read.text()
        );
        assert_eq!(
            read.string("/FunctionArn"),
            Some(expected_arn(function, case.decoded_qualifier)),
            "case {:?}: the read must report the qualifier's ARN, got {}",
            case.name,
            read.text()
        );
        assert_eq!(
            read.at("/RequestedFunctionScalingConfig"),
            json!({ "MinExecutionEnvironments": SERVICE_MANAGED_MINIMUM }),
            "case {:?}: a served qualifier reports what the service scales it by, got {}",
            case.name,
            read.text()
        );
        assert_eq!(
            written.status,
            202,
            "case {:?}: PutFunctionScalingConfig must answer 202, got {} {}",
            case.name,
            written.status,
            written.text()
        );
    }
}

/// The refusal comes AFTER resolution, so it never masks the modelled 404: a name that
/// names no function, and a version number no function published, are refused
/// `ResourceNotFoundException` — with the service's canonical sentence — whether or not the
/// function runs on a capacity provider.
#[tokio::test]
async fn an_unresolvable_request_is_still_refused_the_modelled_404() {
    for provider in [None, Some(FLEET)] {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let function = "one-version";
        create_provider(&router, FLEET).await;
        create_function_with_versions(&router, &engine, function, provider, 1).await;
        let case = provider.unwrap_or("no provider");

        // A version number the function has not published, including the one just past its
        // highest, and one no function ever carries.
        for qualifier in ["2", "7", "0"] {
            for (operation, answer) in [
                (
                    "GetFunctionScalingConfig",
                    get_scaling(&router, function, qualifier).await,
                ),
                (
                    "PutFunctionScalingConfig",
                    put_scaling(&router, function, qualifier, (1, 9)).await,
                ),
            ] {
                assert_eq!(
                    answer.status,
                    404,
                    "case {case:?}: {operation} on the unpublished qualifier {qualifier:?} must \
                     be refused 404, got {} {}",
                    answer.status,
                    answer.text()
                );
                assert!(
                    answer.error_code().contains("ResourceNotFoundException"),
                    "case {case:?}: {operation} on the unpublished qualifier {qualifier:?} must \
                     be refused ResourceNotFoundException, got {} {}",
                    answer.error_code(),
                    answer.text()
                );
                assert_eq!(
                    answer.string("/Message").as_deref(),
                    Some(RESOURCE_NOT_FOUND_MESSAGE),
                    "case {case:?}: {operation} on the unpublished qualifier {qualifier:?} must \
                     carry the canonical sentence, got {}",
                    answer.text()
                );
            }
        }

        // A function nobody created, on each of the two qualifier forms.
        for qualifier in [LATEST_PUBLISHED, "1"] {
            for (operation, answer) in [
                (
                    "GetFunctionScalingConfig",
                    get_scaling(&router, "no-such-function", qualifier).await,
                ),
                (
                    "PutFunctionScalingConfig",
                    put_scaling(&router, "no-such-function", qualifier, (1, 9)).await,
                ),
            ] {
                assert_eq!(
                    answer.status,
                    404,
                    "case {case:?}: {operation} on an unknown function ({qualifier:?}) must be \
                     refused 404, got {} {}",
                    answer.status,
                    answer.text()
                );
                assert_eq!(
                    answer.string("/Message").as_deref(),
                    Some(RESOURCE_NOT_FOUND_MESSAGE),
                    "case {case:?}: {operation} on an unknown function ({qualifier:?}) must carry \
                     the canonical sentence, got {}",
                    answer.text()
                );
            }
        }
    }
}

/// The configuration the refusal reads is the QUALIFIER's own: a version published before a
/// capacity provider was added to the function froze a configuration that names none, so it
/// stays refused while the published view of the updated configuration is served. The
/// refused write stored nothing, so what the served qualifier reports is the
/// service-managed configuration and not the bounds that write asked for.
#[tokio::test]
async fn each_qualifier_is_judged_by_the_configuration_it_froze() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "late-managed";
    create_provider(&router, FLEET).await;
    create_function_with_versions(&router, &engine, function, None, 1).await;

    let refused = put_scaling(&router, function, LATEST_PUBLISHED, (1, 9)).await;
    assert_refused(
        &refused,
        "before the provider is added",
        "PutFunctionScalingConfig",
    );

    let updated = call(
        &router,
        "PUT",
        &format!("{FUNCTIONS_URI}/{function}/configuration"),
        &json!({ "CapacityProviderConfig": capacity_provider_config(FLEET) }),
    )
    .await;
    assert_eq!(
        updated.status,
        200,
        "UpdateFunctionConfiguration must add the capacity provider, got {} {}",
        updated.status,
        updated.text()
    );
    make_activation_due(&engine);

    let served = get_scaling(&router, function, LATEST_PUBLISHED).await;
    assert_eq!(
        served.status,
        200,
        "the updated configuration must be served, got {} {}",
        served.status,
        served.text()
    );
    assert_eq!(
        served.at("/RequestedFunctionScalingConfig"),
        json!({ "MinExecutionEnvironments": SERVICE_MANAGED_MINIMUM }),
        "the refused write must have stored nothing, so the service-managed configuration is \
         what is reported, got {}",
        served.text()
    );

    let frozen = get_scaling(&router, function, "1").await;
    assert_refused(
        &frozen,
        "the version published before the provider was added",
        "GetFunctionScalingConfig",
    );
}
