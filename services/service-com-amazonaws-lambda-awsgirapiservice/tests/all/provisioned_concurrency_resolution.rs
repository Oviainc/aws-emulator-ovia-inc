// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how `GetProvisionedConcurrencyConfig` and
//! `PutProvisionedConcurrencyConfig` RESOLVE the resource they are addressed to.
//!
//! The rule under test: a provisioned concurrency configuration is held by ONE qualifier
//! of one function, so every documented way of naming that qualifier addresses the same
//! single record — the `FunctionName` may be a bare name, a `function:`-prefixed name, a
//! partial ARN or a full ARN, and the qualifier may be carried by the name as well as by
//! the required `Qualifier` parameter — while two different qualifiers of one function
//! (an alias and the version it invokes among them) are two different records.
//!
//! `provisioned_concurrency.rs` pins what a configuration reports and when; this suite
//! pins that WHICH record a request reaches depends only on the qualifier it names and
//! not on the form it names the function by. Its rows differ along that form (all four
//! documented ones, each with and without the qualifier in the name) and along the
//! qualifier (a published version, an alias of it). Nothing here sleeps: the allocation's
//! due instant is moved into the past, which is what waiting would do.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

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

/// The `ProvisionedConcurrencyStatusEnum` value a completed allocation carries, from
/// `smithy_query(query="shape", shape="ProvisionedConcurrencyStatusEnum")`:
/// `IN_PROGRESS | READY | FAILED`.
const STATUS_READY: &str = "READY";

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

/// The provisioned concurrency resource of the function `reference` names, for the
/// qualifier `qualifier` names.
fn concurrency_uri(reference: &str, qualifier: &str) -> String {
    format!("/2019-09-30/functions/{reference}/provisioned-concurrency?Qualifier={qualifier}")
}

/// `PutProvisionedConcurrencyConfig`: allocate `requested` for one qualifier.
async fn put_config(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: &str,
    requested: i64,
) -> Answer {
    call(
        router,
        "PUT",
        &concurrency_uri(reference, qualifier),
        &json!({ "ProvisionedConcurrentExecutions": requested }),
    )
    .await
}

/// `GetProvisionedConcurrencyConfig` on one qualifier.
async fn get_config(router: &MultiServiceRouter, reference: &str, qualifier: &str) -> Answer {
    call(
        router,
        "GET",
        &concurrency_uri(reference, qualifier),
        &Value::Null,
    )
    .await
}

/// Create a function with its first version published, and bring it out of the `Pending`
/// state creation leaves it in.
async fn create_published_function(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
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
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Publish": true,
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
    settle_activation(engine);
}

/// `CreateAlias`: one alias of `function`, named `alias`, invoking `version`.
async fn create_alias(router: &MultiServiceRouter, function: &str, alias: &str, version: &str) {
    let answer = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/aliases"),
        &json!({ "Name": alias, "FunctionVersion": version }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateAlias {alias:?} -> {version:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// Move every stored function's activation instant into the past, so the next call sees
/// the `Pending` -> `Active` transition the clock owes.
fn settle_activation(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
}

/// Move every stored allocation's warm-up instant into the past, so the next read
/// materialises the `IN_PROGRESS` -> `READY` transition the clock owes.
fn make_allocation_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        for config in function.provisioned_concurrency_configs.values_mut() {
            config.ready_due = Some(past);
        }
    }
}

/// Every documented `FunctionName` form of `function`, with the qualifier carried by the
/// name where the form admits one, from the member's own documentation: "Function name –
/// my-function (name-only), my-function:v1 (with alias). Function ARN –
/// arn:aws:lambda:us-west-2:123456789012:function:my-function. Partial ARN –
/// 123456789012:function:my-function."
fn name_forms(function: &str, qualifier: &str) -> Vec<(String, String)> {
    vec![
        ("the bare name".to_string(), function.to_string()),
        (
            "the name with the function: prefix".to_string(),
            format!("function:{function}"),
        ),
        (
            "the name qualified with the qualifier".to_string(),
            format!("{function}:{qualifier}"),
        ),
        (
            "the partial ARN".to_string(),
            format!("{ACCOUNT}:function:{function}"),
        ),
        (
            "the full ARN".to_string(),
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{function}"),
        ),
        (
            "the full ARN qualified with the qualifier".to_string(),
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{function}:{qualifier}"),
        ),
    ]
}

/// The members a reader reports for one configuration, as a comparable value.
fn reported(answer: &Answer) -> Value {
    let body = answer.json();
    json!({
        "Requested": body["RequestedProvisionedConcurrentExecutions"].clone(),
        "Allocated": body["AllocatedProvisionedConcurrentExecutions"].clone(),
        "Available": body["AvailableProvisionedConcurrentExecutions"].clone(),
        "Status": body["Status"].clone(),
        "LastModified": body["LastModified"].clone(),
    })
}

/// Every documented way of naming one qualifier reaches the one record that qualifier
/// holds — on the writing side as well as the reading side.
#[tokio::test]
async fn every_documented_name_form_reaches_one_configuration() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "resolution-subject";
    create_published_function(&router, &engine, function).await;
    create_alias(&router, function, "live", "1").await;

    // One published version and one alias of it: two qualifiers, each addressed by every
    // documented form of the function's name.
    for qualifier in ["1", "live"] {
        // Allocate through the bare name, then read through every form.
        let put = put_config(&router, function, qualifier, 6).await;
        assert_eq!(
            put.status, 202,
            "case {qualifier:?}: expected 202 from PutProvisionedConcurrencyConfig, got {} {}",
            put.status,
            put.text()
        );
        make_allocation_due(&engine);
        let canonical = get_config(&router, function, qualifier).await;
        assert_eq!(
            canonical.status, 200,
            "case {qualifier:?}: expected 200 from GetProvisionedConcurrencyConfig, got {} {}",
            canonical.status,
            canonical.text()
        );
        assert_eq!(
            reported(&canonical),
            json!({
                "Requested": 6,
                "Allocated": 6,
                "Available": 6,
                "Status": STATUS_READY,
                "LastModified": canonical.json()["LastModified"].clone(),
            }),
            "case {qualifier:?}: the configuration must report the amount allocated for it, \
             got {}",
            canonical.text()
        );

        for (form, reference) in name_forms(function, qualifier) {
            let answer = get_config(&router, &reference, qualifier).await;
            assert_eq!(
                answer.status, 200,
                "case {qualifier:?} by {form}: expected 200 from \
                 GetProvisionedConcurrencyConfig, got {} {}",
                answer.status,
                answer.text()
            );
            assert_eq!(
                reported(&answer),
                reported(&canonical),
                "case {qualifier:?} by {form}: every name form must read the one record the \
                 qualifier holds, got {}",
                answer.text()
            );
        }

        // ... and the writing side resolves the same way: an allocation made through one
        // form replaces what the qualifier holds, whichever form reads it back.
        for (index, (form, reference)) in name_forms(function, qualifier).into_iter().enumerate() {
            let amount = 10 + i64::try_from(index).unwrap_or(0);
            let written = put_config(&router, &reference, qualifier, amount).await;
            assert_eq!(
                written.status, 202,
                "case {qualifier:?} by {form}: expected 202 from \
                 PutProvisionedConcurrencyConfig, got {} {}",
                written.status,
                written.text()
            );
            make_allocation_due(&engine);
            let read = get_config(&router, function, qualifier).await;
            assert_eq!(
                reported(&read),
                json!({
                    "Requested": amount,
                    "Allocated": amount,
                    "Available": amount,
                    "Status": STATUS_READY,
                    "LastModified": read.json()["LastModified"].clone(),
                }),
                "case {qualifier:?} by {form}: an allocation made through one name form must \
                 be the one the qualifier holds, got {}",
                read.text()
            );
        }
    }
}

/// An alias and the version it invokes are two qualifiers, so they hold two records: a
/// request resolves by the qualifier it names and by nothing else.
#[tokio::test]
async fn an_alias_and_the_version_it_invokes_hold_two_configurations() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "resolution-alias";
    create_published_function(&router, &engine, function).await;
    create_alias(&router, function, "live", "1").await;

    // Different amounts, so neither qualifier could report the other's by accident.
    let allocations: &[(&str, i64)] = &[("1", 4), ("live", 9)];
    for (qualifier, amount) in allocations {
        let put = put_config(&router, function, qualifier, *amount).await;
        assert_eq!(
            put.status, 202,
            "case {qualifier:?}: expected 202 from PutProvisionedConcurrencyConfig, got {} {}",
            put.status,
            put.text()
        );
    }
    make_allocation_due(&engine);
    for (qualifier, amount) in allocations {
        let answer = get_config(&router, function, qualifier).await;
        assert_eq!(
            answer.json()["RequestedProvisionedConcurrentExecutions"],
            json!(amount),
            "case {qualifier:?}: expected the {amount} allocated for this qualifier, got {}",
            answer.text()
        );
    }

    // The alias's own record is unchanged by a call on the version it invokes, and the
    // qualified name form reaches the same alias record as the parameter does.
    let replaced = put_config(&router, function, "1", 7).await;
    assert_eq!(
        replaced.status, 202,
        "case \"replace the version's allocation\": expected 202, got {} {}",
        replaced.status,
        replaced.text()
    );
    make_allocation_due(&engine);
    let by_parameter = get_config(&router, function, "live").await;
    let by_name = get_config(&router, &format!("{function}:live"), "live").await;
    assert_eq!(
        by_parameter.json()["RequestedProvisionedConcurrentExecutions"],
        json!(9),
        "case \"the alias after the version changed\": the alias keeps its own allocation, \
         got {}",
        by_parameter.text()
    );
    assert_eq!(
        reported(&by_name),
        reported(&by_parameter),
        "case \"the alias by a qualified name\": the qualified name must reach the alias's own \
         record, got {}",
        by_name.text()
    );
}
