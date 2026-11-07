// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a function's scaling configuration:
//! `PutFunctionScalingConfig`, which "Sets the scaling configuration for a Lambda Managed
//! Instances function", and `GetFunctionScalingConfig`, which reports it.
//!
//! The rule under test: a scaling configuration is ONE pair of execution-environment
//! bounds per QUALIFIER of a function — a published version number, or the published view
//! of the unpublished configuration (`$LATEST.PUBLISHED`) — the service-managed one until a
//! `PutFunctionScalingConfig` call sets one on THAT qualifier, from then on exactly the
//! bounds of the most recent such call on it (replacing rather than merging), reported as
//! the REQUESTED configuration while that request is still outstanding and as the APPLIED
//! one ("the actual scaling settings in effect") — and as that one alone — once the clock
//! has made the request due. A qualifier that names no
//! published version, and a name that names no function, are refused
//! `ResourceNotFoundException` (HTTP 404); a configuration whose minimum exceeds its
//! maximum is refused `InvalidParameterValueException` (HTTP 400); a bound outside the
//! modelled range is refused HTTP 400; and every refusal stores nothing.
//!
//! Every row drives real HTTP requests through the router and reads the configuration back
//! through `GetFunctionScalingConfig`, so a value that was answered but never stored fails
//! here. The rows differ along what the rule generalises over: the bounds themselves (the
//! modelled range's published minimum and maximum, values between, equal bounds, one bound
//! alone, neither), the kind of qualifier (the published view of the unpublished
//! configuration and published version numbers, written raw and percent-encoded), the
//! documented `FunctionName` form the requests address the function by, and how many calls
//! have been made (none, one, several). Every function here runs on a capacity provider,
//! because that is the only kind of function these two operations serve; the refusal of
//! one that runs on none is covered by `function_scaling_config_capacity_provider.rs`. The
//! clock-driven half is exercised by moving the stored due instant into the past, so
//! nothing here sleeps.

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

/// The capacity provider every function here runs on: both function-scaling operations
/// serve a Lambda Managed Instances function, so every function under test names one.
const FLEET: &str = "fleet-scaled";

/// The published view of the unpublished configuration, one of the two forms the model's
/// `PublishedFunctionQualifier` admits
/// (`smithy_query(query="shape", shape="PublishedFunctionQualifier")`:
/// `^(\$LATEST\.PUBLISHED|[0-9]+)$`).
const LATEST_PUBLISHED: &str = "$LATEST.PUBLISHED";

/// The published bounds of each member of `FunctionScalingConfig`, from
/// `smithy_query(query="shape", shape="FunctionScalingConfig")`: both
/// `MinExecutionEnvironments` and `MaxExecutionEnvironments` carry `range 0..15000`.
/// Written as the published numbers rather than as a crate constant, so a wrong bound
/// fails here.
const ENVIRONMENTS_MIN: i64 = 0;
/// See [`ENVIRONMENTS_MIN`].
const ENVIRONMENTS_MAX: i64 = 15_000;

/// The `State` the unpublished configuration of the function every row creates reports
/// once its activation is due, from `smithy_query(query="shape", shape="State")`: a
/// function that runs on a capacity provider settles into the non-invocable one.
const STATE_ACTIVE_NON_INVOCABLE: &str = "ActiveNonInvocable";

/// The minimum number of execution environments the service itself keeps a Managed
/// Instances function's qualifier at while no call has asked for anything else. Written as
/// the number itself rather than as a crate constant, so a changed floor fails here; it
/// sits inside the modelled `0..15000` range of both `FunctionScalingConfig` members.
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

    /// The string at `pointer`, when the body carries one there.
    fn string(&self, pointer: &str) -> Option<String> {
        self.json()
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The value at `pointer`, or `Value::Null` when the body carries none there.
    fn at(&self, pointer: &str) -> Value {
        self.json().pointer(pointer).cloned().unwrap_or(Value::Null)
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
        headers: parts.headers,
        body,
    }
}

/// Where both function-scaling operations of one function are bound.
fn scaling_uri(reference: &str, qualifier: &str) -> String {
    format!("/2025-11-30/functions/{reference}/function-scaling-config?Qualifier={qualifier}")
}

/// The `FunctionScalingConfig` body a pair of bounds makes: a member the pair leaves out
/// is left out of the request, because the model gives neither a default.
fn scaling_body(bounds: Bounds) -> Value {
    let mut config = json!({});
    if let Some(minimum) = bounds.0 {
        config["MinExecutionEnvironments"] = json!(minimum);
    }
    if let Some(maximum) = bounds.1 {
        config["MaxExecutionEnvironments"] = json!(maximum);
    }
    json!({ "FunctionScalingConfig": config })
}

/// `PutFunctionScalingConfig`
/// (`PUT /2025-11-30/functions/{FunctionName}/function-scaling-config?Qualifier=`).
async fn put_scaling(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: &str,
    bounds: Bounds,
) -> Answer {
    call(
        router,
        "PUT",
        &scaling_uri(reference, qualifier),
        &scaling_body(bounds),
    )
    .await
}

/// `GetFunctionScalingConfig` (`GET` of the same resource).
async fn get_scaling(router: &MultiServiceRouter, reference: &str, qualifier: &str) -> Answer {
    call(
        router,
        "GET",
        &scaling_uri(reference, qualifier),
        &Value::Null,
    )
    .await
}

/// The ARN a capacity provider of `name` carries, as the model's `CapacityProviderArn`
/// pattern spells it.
fn provider_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider:{name}")
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

/// Create a Managed Instances function with `versions` published versions — the only kind
/// of function the function-scaling operations serve — and bring it out of the `Pending`
/// state creation leaves it in.
async fn create_function_with_versions(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
    versions: u32,
) {
    create_provider(router, FLEET).await;
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Publish": versions >= 1,
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
            "CapacityProviderConfig": {
                "LambdaManagedInstancesCapacityProviderConfig": {
                    "CapacityProviderArn": provider_arn(FLEET),
                },
            },
        }),
    )
    .await;
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

/// The bounds one call states: the minimum and the maximum, either of which the request
/// may leave out.
type Bounds = (Option<i64>, Option<i64>);

/// The `FunctionScalingConfig` a stored pair of bounds must report back: the members the
/// call stated, and no others.
fn reported(bounds: Bounds) -> Value {
    let mut config = json!({});
    if let Some(minimum) = bounds.0 {
        config["MinExecutionEnvironments"] = json!(minimum);
    }
    if let Some(maximum) = bounds.1 {
        config["MaxExecutionEnvironments"] = json!(maximum);
    }
    config
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

/// One row of the round-trip table: the qualifier the configuration is set on, the
/// `FunctionName` form the requests address the function by, and the bounds set on it in
/// order.
struct Case {
    name: &'static str,
    /// How many versions the function publishes.
    versions: u32,
    /// The qualifier every call under test addresses, as the request writes it.
    qualifier: &'static str,
    /// The qualifier label the answer is reported against, once decoded.
    decoded_qualifier: &'static str,
    /// The `FunctionName` form, built from the function name; `None` means the bare name.
    reference: Option<fn(&str) -> String>,
    /// The bounds `PutFunctionScalingConfig` sets, in order.
    puts: &'static [Bounds],
}

/// The rows. Between them they cover the published bounds of the modelled range (its
/// minimum `0` and its maximum `15000`), values between them, equal bounds, one bound
/// alone and neither, the published view of the unpublished configuration and a published
/// version number as the qualifier (written raw and percent-encoded), each documented
/// `FunctionName` form, and a call count of one and several.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "published-view-of-latest, bare name, one call, the range's bounds",
            versions: 0,
            qualifier: LATEST_PUBLISHED,
            decoded_qualifier: LATEST_PUBLISHED,
            reference: None,
            puts: &[(Some(ENVIRONMENTS_MIN), Some(ENVIRONMENTS_MAX))],
        },
        Case {
            name: "published-view-of-latest percent-encoded, full ARN, values between",
            versions: 0,
            qualifier: "%24LATEST.PUBLISHED",
            decoded_qualifier: LATEST_PUBLISHED,
            reference: Some(|name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")),
            puts: &[(Some(2), Some(40))],
        },
        Case {
            name: "version 1, partial ARN, equal bounds",
            versions: 1,
            qualifier: "1",
            decoded_qualifier: "1",
            reference: Some(|name| format!("{ACCOUNT}:function:{name}")),
            puts: &[(Some(7), Some(7))],
        },
        Case {
            name: "version 2, name with the resource-type prefix, several calls replace",
            versions: 2,
            qualifier: "2",
            decoded_qualifier: "2",
            reference: Some(|name| format!("function:{name}")),
            puts: &[(Some(1), Some(9)), (Some(0), Some(3)), (Some(5), Some(500))],
        },
        Case {
            name: "published-view-of-latest, the maximum alone",
            versions: 0,
            qualifier: LATEST_PUBLISHED,
            decoded_qualifier: LATEST_PUBLISHED,
            reference: None,
            puts: &[(None, Some(12))],
        },
        Case {
            name: "version 1, the minimum alone, then both",
            versions: 1,
            qualifier: "1",
            decoded_qualifier: "1",
            reference: None,
            puts: &[(Some(3), None), (Some(4), Some(4))],
        },
        Case {
            name: "published-view-of-latest, neither bound stated",
            versions: 0,
            qualifier: LATEST_PUBLISHED,
            decoded_qualifier: LATEST_PUBLISHED,
            reference: None,
            puts: &[(None, None)],
        },
    ]
}

/// Every row: the bounds the last call stated are what the qualifier reports back, as the
/// requested configuration while the clock has not applied them yet and as the applied one
/// afterwards.
#[tokio::test]
async fn stored_bounds_round_trip_through_get() {
    for case in cases() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let function = "scaled-function";
        create_function_with_versions(&router, &engine, function, case.versions).await;
        let reference = case
            .reference
            .map_or_else(|| function.to_string(), |build| build(function));

        // Nothing a CALL has asked for until one sets something: the qualifier is scaled
        // by the service, so the read succeeds and reports the service-managed floor.
        let before = get_scaling(&router, &reference, case.qualifier).await;
        assert_eq!(
            before.status,
            200,
            "case {:?}: GetFunctionScalingConfig before any call must answer 200, got {} {}",
            case.name,
            before.status,
            before.text()
        );
        assert_eq!(
            before.at("/RequestedFunctionScalingConfig"),
            json!({ "MinExecutionEnvironments": SERVICE_MANAGED_MINIMUM }),
            "case {:?}: an unconfigured qualifier must report the service-managed \
             configuration, got {}",
            case.name,
            before.text()
        );
        assert_eq!(
            before.at("/AppliedFunctionScalingConfig"),
            Value::Null,
            "case {:?}: an unconfigured qualifier must report no applied configuration, got {}",
            case.name,
            before.text()
        );
        assert_eq!(
            before.string("/FunctionArn"),
            Some(expected_arn(function, case.decoded_qualifier)),
            "case {:?}: the read must report the qualifier's ARN, got {}",
            case.name,
            before.text()
        );

        for bounds in case.puts {
            let answer = put_scaling(&router, &reference, case.qualifier, *bounds).await;
            assert_eq!(
                answer.status,
                202,
                "case {:?}: PutFunctionScalingConfig {bounds:?} must answer 202, got {} {}",
                case.name,
                answer.status,
                answer.text()
            );
            assert_eq!(
                answer.string("/FunctionState").as_deref(),
                Some(STATE_ACTIVE_NON_INVOCABLE),
                "case {:?}: the answer must report the function's own state, got {}",
                case.name,
                answer.text()
            );
        }

        let last = *case.puts.last().expect("every row states at least one call");

        // Requested at once, applied only once the clock owes it.
        let requested = get_scaling(&router, &reference, case.qualifier).await;
        assert_eq!(
            requested.status,
            200,
            "case {:?}: GetFunctionScalingConfig must answer 200, got {} {}",
            case.name,
            requested.status,
            requested.text()
        );
        assert_eq!(
            requested.at("/RequestedFunctionScalingConfig"),
            reported(last),
            "case {:?}: expected the last call's bounds {last:?} reported as requested, got {}",
            case.name,
            requested.text()
        );
        assert_eq!(
            requested.at("/AppliedFunctionScalingConfig"),
            Value::Null,
            "case {:?}: bounds the clock has not applied yet must not be reported as applied, \
             got {}",
            case.name,
            requested.text()
        );

        make_scaling_applied(&engine);
        let applied = get_scaling(&router, &reference, case.qualifier).await;
        assert_eq!(
            applied.at("/AppliedFunctionScalingConfig"),
            reported(last),
            "case {:?}: expected the last call's bounds {last:?} reported as applied once due, \
             got {}",
            case.name,
            applied.text()
        );
        assert_eq!(
            applied.at("/RequestedFunctionScalingConfig"),
            Value::Null,
            "case {:?}: bounds the clock has applied leave no request outstanding, so none \
             must be reported as requested, got {}",
            case.name,
            applied.text()
        );
        assert_eq!(
            applied.string("/FunctionArn"),
            Some(expected_arn(function, case.decoded_qualifier)),
            "case {:?}: the read must report the qualifier's ARN, got {}",
            case.name,
            applied.text()
        );
    }
}

/// The configuration belongs to the QUALIFIER: each of a function's qualifiers holds its
/// own bounds, and setting one leaves the others alone.
#[tokio::test]
async fn each_qualifier_holds_its_own_bounds() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "many-qualifiers";
    create_function_with_versions(&router, &engine, function, 2).await;

    let rows: &[(&str, Bounds)] = &[
        (LATEST_PUBLISHED, (Some(1), Some(2))),
        ("1", (Some(10), Some(20))),
        ("2", (Some(100), Some(200))),
    ];
    for (qualifier, bounds) in rows {
        let answer = put_scaling(&router, function, qualifier, *bounds).await;
        assert_eq!(
            answer.status,
            202,
            "qualifier {qualifier:?}: PutFunctionScalingConfig {bounds:?} must answer 202, \
             got {} {}",
            answer.status,
            answer.text()
        );
    }
    make_scaling_applied(&engine);
    for (qualifier, bounds) in rows {
        let answer = get_scaling(&router, function, qualifier).await;
        assert_eq!(
            answer.at("/AppliedFunctionScalingConfig"),
            reported(*bounds),
            "qualifier {qualifier:?}: expected its own bounds {bounds:?}, got {}",
            answer.text()
        );
    }
}

/// A configuration whose minimum is above its maximum describes no range, and a bound
/// outside the modelled range is no bound: both are refused and neither stores anything.
#[tokio::test]
async fn bounds_that_describe_no_range_are_refused_and_store_nothing() {
    // The rows: the refused bounds, and whether the refusal is the handler's own modelled
    // 400 (`InvalidParameterValueException`) or the boundary's refusal of a value outside
    // the modelled `0..15000` range, which carries its own code.
    let rows: &[(Bounds, bool)] = &[
        ((Some(1), Some(ENVIRONMENTS_MIN)), true),
        ((Some(10), Some(9)), true),
        ((Some(ENVIRONMENTS_MAX), Some(1)), true),
        ((Some(ENVIRONMENTS_MAX), Some(ENVIRONMENTS_MAX - 1)), true),
        ((Some(ENVIRONMENTS_MIN - 1), Some(5)), false),
        ((Some(1), Some(ENVIRONMENTS_MAX + 1)), false),
    ];
    for (bounds, modelled) in rows {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let function = "refused-bounds";
        create_function_with_versions(&router, &engine, function, 1).await;

        for qualifier in [LATEST_PUBLISHED, "1"] {
            let answer = put_scaling(&router, function, qualifier, *bounds).await;
            assert_eq!(
                answer.status,
                400,
                "bounds {bounds:?} on {qualifier:?} must be refused 400, got {} {}",
                answer.status,
                answer.text()
            );
            if *modelled {
                assert!(
                    answer.error_code().contains("InvalidParameterValueException"),
                    "bounds {bounds:?} on {qualifier:?} must be refused \
                     InvalidParameterValueException, got {} {}",
                    answer.error_code(),
                    answer.text()
                );
                // The refusal is a FIXED sentence naming the two bounds' roles, with no
                // part of it taken from the request: it is the one AWS words for a
                // configuration that describes no range.
                let reported = answer
                    .string("/Message")
                    .or_else(|| answer.string("/message"))
                    .unwrap_or_default();
                assert_eq!(
                    reported,
                    "Invalid function scaling configuration. Min execution environments \
                     should be lower or equal than Max execution environments.",
                    "bounds {bounds:?} on {qualifier:?}: expected the fixed range sentence, \
                     got {reported:?}"
                );
            }

            // Nothing was stored by the refusal: the qualifier still reports what the
            // service, and no call, manages for it.
            let read = get_scaling(&router, function, qualifier).await;
            assert_eq!(
                read.at("/RequestedFunctionScalingConfig"),
                json!({ "MinExecutionEnvironments": SERVICE_MANAGED_MINIMUM }),
                "bounds {bounds:?} on {qualifier:?} must store nothing, got {}",
                read.text()
            );
        }
    }
}

/// A qualifier that names no published version, and a name that names no function, name
/// no resource a configuration can be set on or read from.
#[tokio::test]
async fn unknown_qualifiers_and_functions_are_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "one-version";
    create_function_with_versions(&router, &engine, function, 1).await;

    // A version number the function has not published, including the one just past its
    // highest, and a number no function ever carries.
    for qualifier in ["2", "7", "99", "0"] {
        let written = put_scaling(&router, function, qualifier, (Some(1), Some(2))).await;
        assert_eq!(
            written.status,
            404,
            "PutFunctionScalingConfig on unpublished qualifier {qualifier:?} must be refused \
             404, got {} {}",
            written.status,
            written.text()
        );
        assert!(
            written.error_code().contains("ResourceNotFoundException"),
            "qualifier {qualifier:?} must be refused ResourceNotFoundException, got {} {}",
            written.error_code(),
            written.text()
        );
        let read = get_scaling(&router, function, qualifier).await;
        assert_eq!(
            read.status,
            404,
            "GetFunctionScalingConfig on unpublished qualifier {qualifier:?} must be refused \
             404, got {} {}",
            read.status,
            read.text()
        );
    }

    // A function nobody created, on each of the two qualifier forms.
    for qualifier in [LATEST_PUBLISHED, "1"] {
        let written = put_scaling(&router, "no-such-function", qualifier, (Some(1), Some(2))).await;
        assert_eq!(
            written.status,
            404,
            "PutFunctionScalingConfig on an unknown function ({qualifier:?}) must be refused \
             404, got {} {}",
            written.status,
            written.text()
        );
        let read = get_scaling(&router, "no-such-function", qualifier).await;
        assert_eq!(
            read.status,
            404,
            "GetFunctionScalingConfig on an unknown function ({qualifier:?}) must be refused \
             404, got {} {}",
            read.status,
            read.text()
        );
    }
}

/// The applied bounds change only when the clock makes the change due: a second call
/// replaces what is requested at once, while what is in effect stays the previous one
/// until then.
#[tokio::test]
async fn applied_bounds_follow_the_clock() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "clock-driven";
    create_function_with_versions(&router, &engine, function, 0).await;

    let first: Bounds = (Some(1), Some(10));
    let second: Bounds = (Some(20), Some(30));

    put_scaling(&router, function, LATEST_PUBLISHED, first).await;
    make_scaling_applied(&engine);
    let settled = get_scaling(&router, function, LATEST_PUBLISHED).await;
    assert_eq!(
        settled.at("/AppliedFunctionScalingConfig"),
        reported(first),
        "the first bounds {first:?} must be in effect once their application is due, got {}",
        settled.text()
    );

    put_scaling(&router, function, LATEST_PUBLISHED, second).await;
    let pending = get_scaling(&router, function, LATEST_PUBLISHED).await;
    assert_eq!(
        pending.at("/RequestedFunctionScalingConfig"),
        reported(second),
        "the second call's bounds {second:?} must be the requested ones at once, got {}",
        pending.text()
    );
    assert_eq!(
        pending.at("/AppliedFunctionScalingConfig"),
        reported(first),
        "the bounds in effect must still be {first:?} until the change is due, got {}",
        pending.text()
    );

    make_scaling_applied(&engine);
    let replaced = get_scaling(&router, function, LATEST_PUBLISHED).await;
    assert_eq!(
        replaced.at("/AppliedFunctionScalingConfig"),
        reported(second),
        "the second call's bounds {second:?} must be in effect once due, got {}",
        replaced.text()
    );
}
