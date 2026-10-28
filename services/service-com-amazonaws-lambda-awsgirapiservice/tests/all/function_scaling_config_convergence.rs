// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how `GetFunctionScalingConfig` reports a
//! `PutFunctionScalingConfig` request once it has been APPLIED.
//!
//! The rule under test: `RequestedFunctionScalingConfig` is the configuration that is
//! still waiting to take effect, so the response carries it only while a request is
//! outstanding. Once the request has become
//! `AppliedFunctionScalingConfig` — "the scaling configuration that is currently applied
//! to the function ... the actual scaling settings in effect" — there is no outstanding
//! request left, the member is omitted entirely, and its ABSENCE is what tells a client
//! reconciling the bounds that its request landed. A further call re-opens the window: the
//! new bounds are reported as requested again until the clock applies them, while the
//! previous ones stay in effect.
//!
//! Every row drives real HTTP requests through the router and reads the configuration back
//! through `GetFunctionScalingConfig`, so a member that is answered but not stored fails
//! here. The rows differ along what the rule generalises over: the bounds themselves (each
//! published bound of the modelled range, values between, equal bounds, one bound alone,
//! neither), the qualifier the configuration is set on (the published view of the
//! unpublished configuration and published version numbers), and how many requests have
//! been made. The clock-driven half is exercised by moving the stored due instant into the
//! past, so nothing here sleeps.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`,
/// which is what routes a request here and what region the minted ARNs carry. Nothing in
/// it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

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

    /// Whether the body carries a member at `pointer` at all.
    fn has(&self, pointer: &str) -> bool {
        self.json().pointer(pointer).is_some()
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

/// Where both function-scaling operations of one qualifier are bound.
fn scaling_uri(reference: &str, qualifier: &str) -> String {
    format!("/2025-11-30/functions/{reference}/function-scaling-config?Qualifier={qualifier}")
}

/// The bounds one call states: the minimum and the maximum, either of which the request
/// may leave out.
type Bounds = (Option<i64>, Option<i64>);

/// The `FunctionScalingConfig` a pair of bounds makes, as a request body and as the
/// configuration a response must report back: the members the pair states, and no others,
/// because the model gives neither a default.
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

/// `PutFunctionScalingConfig`.
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
        &json!({ "FunctionScalingConfig": reported(bounds) }),
    )
    .await
}

/// `GetFunctionScalingConfig`.
async fn get_scaling(router: &MultiServiceRouter, reference: &str, qualifier: &str) -> Answer {
    call(
        router,
        "GET",
        &scaling_uri(reference, qualifier),
        &Value::Null,
    )
    .await
}

/// Create a function with `versions` published versions, and bring it out of the
/// `Pending` state creation leaves it in.
async fn create_function_with_versions(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
    versions: u32,
) {
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
            "Runtime": "python3.13",
            "Handler": "index.handler",
            "Publish": versions >= 1,
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
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
/// materialises the `Pending` -> `Active` transition the clock owes.
fn make_activation_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
}

/// Move every outstanding scaling request's application instant into the past, so the next
/// read materialises the requested -> applied move the clock owes. This is what waiting
/// would do, without waiting.
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

/// One row: the qualifier the configuration is set on, and the bounds each call asks for,
/// in order.
struct Case {
    name: &'static str,
    /// How many versions the function publishes.
    versions: u32,
    /// The qualifier every call under test addresses.
    qualifier: &'static str,
    /// The bounds each `PutFunctionScalingConfig` asks for, in order.
    puts: &'static [Bounds],
}

/// The rows. Between them they cover each published bound of the modelled range, values
/// between them, equal bounds, one bound alone and neither, the published view of the
/// unpublished configuration and two published version numbers as the qualifier, and a
/// request count of one and several.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "published-view-of-latest, one request, the range's bounds",
            versions: 0,
            qualifier: LATEST_PUBLISHED,
            puts: &[(Some(ENVIRONMENTS_MIN), Some(ENVIRONMENTS_MAX))],
        },
        Case {
            name: "version 1, one request, values between the bounds",
            versions: 1,
            qualifier: "1",
            puts: &[(Some(1), Some(2))],
        },
        Case {
            name: "version 2, equal bounds",
            versions: 2,
            qualifier: "2",
            puts: &[(Some(7), Some(7))],
        },
        Case {
            name: "published-view-of-latest, the maximum alone",
            versions: 0,
            qualifier: LATEST_PUBLISHED,
            puts: &[(None, Some(12))],
        },
        Case {
            name: "version 1, the minimum alone",
            versions: 1,
            qualifier: "1",
            puts: &[(Some(ENVIRONMENTS_MAX), None)],
        },
        Case {
            name: "published-view-of-latest, neither bound stated",
            versions: 0,
            qualifier: LATEST_PUBLISHED,
            puts: &[(None, None)],
        },
        Case {
            name: "version 1, three requests, the last one applied",
            versions: 1,
            qualifier: "1",
            puts: &[(Some(1), Some(9)), (Some(0), Some(3)), (Some(5), Some(500))],
        },
    ]
}

/// Every row: while the last request is outstanding the response carries it as the
/// requested configuration; once the clock has applied it the member is gone and the
/// response carries the ARN and the applied configuration alone.
#[tokio::test]
async fn an_applied_request_is_no_longer_reported_as_requested() {
    for case in cases() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let function = "converging-function";
        create_function_with_versions(&router, &engine, function, case.versions).await;

        for bounds in case.puts {
            let answer = put_scaling(&router, function, case.qualifier, *bounds).await;
            assert_eq!(
                answer.status,
                202,
                "case {:?}: PutFunctionScalingConfig {bounds:?} must answer 202, got {} {}",
                case.name,
                answer.status,
                answer.text()
            );
        }
        let last = *case.puts.last().expect("every row states at least one request");

        // Outstanding: the request is what the response reports, and nothing is in effect
        // yet.
        let outstanding = get_scaling(&router, function, case.qualifier).await;
        assert_eq!(
            outstanding.at("/RequestedFunctionScalingConfig"),
            reported(last),
            "case {:?}: an outstanding request {last:?} must be reported as requested, got {}",
            case.name,
            outstanding.text()
        );

        make_scaling_applied(&engine);

        // Applied: the request is gone from the response, and the bounds it asked for are
        // the ones in effect.
        let applied = get_scaling(&router, function, case.qualifier).await;
        assert_eq!(
            applied.status,
            200,
            "case {:?}: GetFunctionScalingConfig must answer 200, got {} {}",
            case.name,
            applied.status,
            applied.text()
        );
        assert!(
            !applied.has("/RequestedFunctionScalingConfig"),
            "case {:?}: an applied request must leave no RequestedFunctionScalingConfig \
             member at all, got {}",
            case.name,
            applied.text()
        );
        assert_eq!(
            applied.at("/AppliedFunctionScalingConfig"),
            reported(last),
            "case {:?}: expected the applied configuration {last:?}, got {}",
            case.name,
            applied.text()
        );
        assert_eq!(
            applied.json(),
            json!({
                "FunctionArn": expected_arn(function, case.qualifier),
                "AppliedFunctionScalingConfig": reported(last),
            }),
            "case {:?}: expected the ARN and the applied configuration alone, got {}",
            case.name,
            applied.text()
        );

        // A read reports; it stores nothing, so the next caller sees the same answer.
        let again = get_scaling(&router, function, case.qualifier).await;
        assert_eq!(
            again.json(),
            applied.json(),
            "case {:?}: a second read must answer the same thing, got {} then {}",
            case.name,
            applied.text(),
            again.text()
        );
    }
}

/// A further request re-opens the window: it is reported as requested again until the
/// clock applies it, the bounds in effect stay the previous ones until then, and the
/// member disappears once more when it lands.
#[tokio::test]
async fn a_further_request_is_reported_until_it_too_is_applied() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "reconciled-function";
    create_function_with_versions(&router, &engine, function, 0).await;

    let rounds: &[Bounds] = &[(Some(1), Some(2)), (Some(2), Some(40)), (Some(0), None)];
    let mut in_effect: Option<Bounds> = None;
    for bounds in rounds {
        put_scaling(&router, function, LATEST_PUBLISHED, *bounds).await;

        let pending = get_scaling(&router, function, LATEST_PUBLISHED).await;
        assert_eq!(
            pending.at("/RequestedFunctionScalingConfig"),
            reported(*bounds),
            "request {bounds:?}: must be reported as requested while outstanding, got {}",
            pending.text()
        );
        assert_eq!(
            pending.at("/AppliedFunctionScalingConfig"),
            in_effect.map_or(Value::Null, reported),
            "request {bounds:?}: the bounds in effect must still be {in_effect:?}, got {}",
            pending.text()
        );

        make_scaling_applied(&engine);
        let settled = get_scaling(&router, function, LATEST_PUBLISHED).await;
        assert!(
            !settled.has("/RequestedFunctionScalingConfig"),
            "request {bounds:?}: once applied no requested configuration must be reported, \
             got {}",
            settled.text()
        );
        assert_eq!(
            settled.at("/AppliedFunctionScalingConfig"),
            reported(*bounds),
            "request {bounds:?}: expected it in effect once applied, got {}",
            settled.text()
        );
        in_effect = Some(*bounds);
    }
}
