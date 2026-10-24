// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a function's provisioned concurrency:
//! `PutProvisionedConcurrencyConfig` "Adds a provisioned concurrency configuration to a
//! function's alias or version", `GetProvisionedConcurrencyConfig` and
//! `ListProvisionedConcurrencyConfigs`, which report it, and
//! `DeleteProvisionedConcurrencyConfig`, which removes it.
//!
//! The rule under test: a provisioned concurrency configuration is ONE value per
//! QUALIFIER of a function — an alias or a published version — absent until a
//! `PutProvisionedConcurrencyConfig` call adds one to THAT qualifier, from then on the
//! amount of the most recent such call (replacing rather than accumulating), and gone
//! once it is deleted; every reader of that qualifier reports the same stored record, and
//! the allocation it asks for is owed by the CLOCK, so the configuration is `IN_PROGRESS`
//! with nothing allocated until the allocation is due and `READY` with all of it
//! allocated and available afterwards. The unpublished version is neither an alias nor a
//! published version, so it is refused `InvalidParameterValueException` (HTTP 400) on
//! every one of those operations; a qualifier that names no alias or version at all is
//! refused `ResourceNotFoundException` (HTTP 404); a qualifier holding no configuration is
//! refused `ProvisionedConcurrencyConfigNotFoundException` (HTTP 404); and an amount that
//! would take the account past the documented unreserved-capacity floor, or past the
//! function's own reserved concurrency, is refused and stores nothing.
//!
//! Every row drives real HTTP requests through the router and reads the configuration back
//! through `GetProvisionedConcurrencyConfig` and `ListProvisionedConcurrencyConfigs`, so a
//! value that was answered but never stored fails here. The rows differ along what the
//! rule generalises over: the amount allocated (the modelled minimum, values between, the
//! largest the documented floor admits, and one past it), the kind of qualifier (a
//! published version number, an alias, several of each), the documented `FunctionName`
//! form the requests address the function by, and how many calls have been made (none,
//! one, several). The clock-driven half is exercised by moving the stored due instant into
//! the past, so nothing here sleeps.

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

/// The unpublished version label, from the model's `Version` pattern
/// (`^(\$LATEST|[0-9]+)$`).
const VERSION_LATEST: &str = "$LATEST";

/// The `ProvisionedConcurrencyStatusEnum` values an allocation moves through, from
/// `smithy_query(query="shape", shape="ProvisionedConcurrencyStatusEnum")`:
/// `IN_PROGRESS | READY | FAILED`.
const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`STATUS_IN_PROGRESS`].
const STATUS_READY: &str = "READY";

/// `AccountLimit$ConcurrentExecutions`: the published default concurrency quota of an
/// account in a Region (AWS Lambda quotas, "Concurrent executions … 1,000"). Written as
/// the published number rather than as the crate's constant, so a wrong constant fails
/// here.
const CONCURRENT_EXECUTIONS_LIMIT: i64 = 1_000;

/// The capacity an account must leave unreserved, from `PutFunctionConcurrency`'s own
/// documentation: "You can reserve concurrency for as many functions as you like, as long
/// as you leave at least 100 simultaneous executions unreserved for functions that aren't
/// configured with a per-function limit."
const MIN_UNRESERVED: i64 = 100;

/// The largest allocation the rule above admits on an account that commits nothing else.
const MAX_ALLOCATABLE: i64 = CONCURRENT_EXECUTIONS_LIMIT - MIN_UNRESERVED;

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

    /// The integer at `pointer`, when the body carries one there.
    fn number(&self, pointer: &str) -> Option<i64> {
        self.json().pointer(pointer).and_then(Value::as_i64)
    }

    /// The string at `pointer`, when the body carries one there.
    fn string(&self, pointer: &str) -> Option<String> {
        self.json()
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::to_string)
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

/// Where every provisioned concurrency operation of one function is bound.
fn concurrency_uri(reference: &str) -> String {
    format!("/2019-09-30/functions/{reference}/provisioned-concurrency")
}

/// `PutProvisionedConcurrencyConfig`
/// (`PUT /2019-09-30/functions/{FunctionName}/provisioned-concurrency?Qualifier=`).
async fn put_config(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: &str,
    requested: i64,
) -> Answer {
    call(
        router,
        "PUT",
        &format!(
            "{}?Qualifier={qualifier}",
            concurrency_uri(reference),
        ),
        &json!({ "ProvisionedConcurrentExecutions": requested }),
    )
    .await
}

/// `GetProvisionedConcurrencyConfig` (`GET` of the same resource).
async fn get_config(router: &MultiServiceRouter, reference: &str, qualifier: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{}?Qualifier={qualifier}", concurrency_uri(reference)),
        &Value::Null,
    )
    .await
}

/// `DeleteProvisionedConcurrencyConfig` (`DELETE` of the same resource).
async fn delete_config(router: &MultiServiceRouter, reference: &str, qualifier: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("{}?Qualifier={qualifier}", concurrency_uri(reference)),
        &Value::Null,
    )
    .await
}

/// `ListProvisionedConcurrencyConfigs` (`GET` of the collection, `?List=ALL`), with
/// whatever pagination parameters the caller names.
async fn list_configs(router: &MultiServiceRouter, reference: &str, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{}?List=ALL{query}", concurrency_uri(reference)),
        &Value::Null,
    )
    .await
}

/// Create a function to provision concurrency for, with `versions` published versions,
/// and bring it out of the `Pending` state creation leaves it in.
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
            "Runtime": "python3.12",
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

/// Give the function an alias of `name` invoking `version`.
async fn create_alias(router: &MultiServiceRouter, function: &str, name: &str, version: &str) {
    let answer = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/aliases"),
        &json!({ "Name": name, "FunctionVersion": version }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateAlias {name:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// Reserve `reserved` simultaneous executions for the function.
async fn put_reserved_concurrency(router: &MultiServiceRouter, function: &str, reserved: i64) {
    let answer = call(
        router,
        "PUT",
        &format!("/2017-10-31/functions/{function}/concurrency"),
        &json!({ "ReservedConcurrentExecutions": reserved }),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "PutFunctionConcurrency of {reserved} on {function:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
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

/// The ARN of one qualifier of a function, which is what every reader reports for its
/// configuration.
fn qualified_arn(function: &str, qualifier: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{function}:{qualifier}")
}

/// The four members every reader reports for one configuration, as a comparable value.
fn reported(answer: &Answer) -> Value {
    json!({
        "Requested": answer.number("/RequestedProvisionedConcurrentExecutions"),
        "Allocated": answer.number("/AllocatedProvisionedConcurrentExecutions"),
        "Available": answer.number("/AvailableProvisionedConcurrentExecutions"),
        "Status": answer.string("/Status"),
    })
}

/// What a configuration of `requested` must report in each of the two statuses: nothing
/// is allocated while the allocation is still being made, and all of it once it is ready.
fn expected(requested: i64, status: &str) -> Value {
    let allocated = if status == STATUS_READY { requested } else { 0 };
    json!({
        "Requested": requested,
        "Allocated": allocated,
        "Available": allocated,
        "Status": status,
    })
}

/// The entry `ListProvisionedConcurrencyConfigs` reports for one qualifier, or `None`
/// when it reports none.
fn listed(answer: &Answer, function_arn: &str) -> Option<Value> {
    answer
        .json()
        .pointer("/ProvisionedConcurrencyConfigs")
        .and_then(Value::as_array)
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["FunctionArn"] == json!(function_arn))
                .cloned()
        })
        .map(|entry| {
            json!({
                "Requested": entry["RequestedProvisionedConcurrentExecutions"].clone(),
                "Allocated": entry["AllocatedProvisionedConcurrentExecutions"].clone(),
                "Available": entry["AvailableProvisionedConcurrentExecutions"].clone(),
                "Status": entry["Status"].clone(),
            })
        })
}

/// One row of the round-trip table: the qualifier a configuration is added to, the
/// `FunctionName` form the requests address the function by, and the amounts allocated on
/// it in order.
struct Case {
    name: &'static str,
    /// How many versions the function publishes.
    versions: u32,
    /// The alias to create before the calls, and the version it invokes.
    alias: Option<(&'static str, &'static str)>,
    /// The qualifier every call under test addresses.
    qualifier: &'static str,
    /// The `FunctionName` form, built from the function name; `None` means the bare name.
    reference: Option<fn(&str) -> String>,
    /// The amounts `PutProvisionedConcurrencyConfig` asks for, in order.
    puts: &'static [i64],
}

/// The rows. Between them they cover the published bounds of
/// `ProvisionedConcurrentExecutions` on an otherwise empty account (its modelled minimum
/// `1`, the maximum the documented 100-unreserved rule admits, and values between), a
/// qualifier that is a published version number and one that is an alias, each documented
/// `FunctionName` form, and a call count of one and several.
fn cases() -> Vec<Case> {
    vec![
        // The modelled minimum (`@range(min: 1)`) on a published version, addressed by
        // the bare name.
        Case {
            name: "the modelled minimum on a published version",
            versions: 1,
            alias: None,
            qualifier: "1",
            reference: None,
            puts: &[1],
        },
        // An alias, addressed by the `function:` prefixed form.
        Case {
            name: "an alias, addressed by the function: form",
            versions: 1,
            alias: Some(("live", "1")),
            qualifier: "live",
            reference: Some(|name| format!("function:{name}")),
            puts: &[25],
        },
        // The largest allocation the documented floor admits here, addressed by the
        // partial ARN.
        Case {
            name: "the largest allocation the unreserved-capacity rule admits",
            versions: 2,
            alias: None,
            qualifier: "2",
            reference: Some(|name| format!("{ACCOUNT}:function:{name}")),
            puts: &[MAX_ALLOCATABLE],
        },
        // Several calls up and down on one alias: the last one wins, because the
        // configuration is one value of that qualifier rather than a list to add to.
        Case {
            name: "several calls on one alias end on the last amount",
            versions: 1,
            alias: Some(("blue-green_1", "1")),
            qualifier: "blue-green_1",
            reference: Some(|name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")),
            puts: &[10, MAX_ALLOCATABLE, 3],
        },
        // The same amount twice: it REPLACES what the qualifier holds rather than adding
        // to it, so a repeat at the ceiling is still inside the rule. The `FunctionName`
        // carries the qualifier itself here, which the required `Qualifier` parameter
        // agrees with.
        Case {
            name: "the ceiling twice, addressed by a qualified function name",
            versions: 1,
            alias: None,
            qualifier: "1",
            reference: Some(|name| format!("{name}:1")),
            puts: &[MAX_ALLOCATABLE, MAX_ALLOCATABLE],
        },
    ]
}

#[tokio::test]
async fn a_qualifier_reports_the_provisioned_concurrency_it_holds() {
    for case in cases() {
        // One account per row, so each row's allocations are the only ones the
        // account-wide rule has to leave room for.
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let label = case.name;
        let function = "provisioned-subject";
        create_function_with_versions(&router, &engine, function, case.versions).await;
        if let Some((alias, version)) = case.alias {
            create_alias(&router, function, alias, version).await;
        }
        let reference = case
            .reference
            .map_or_else(|| function.to_string(), |build| build(function));
        let qualifier = case.qualifier;

        // Before anything is added, the qualifier holds no configuration at all and the
        // function lists none.
        let initial = get_config(&router, &reference, qualifier).await;
        assert_eq!(
            initial.status,
            404,
            "case {label:?}: a qualifier nobody configured holds nothing, got {} {}",
            initial.status,
            initial.text()
        );
        assert!(
            initial
                .error_code()
                .contains("ProvisionedConcurrencyConfigNotFoundException"),
            "case {label:?}: expected ProvisionedConcurrencyConfigNotFoundException, got {:?}",
            initial.error_code()
        );

        for amount in case.puts {
            let put = put_config(&router, &reference, qualifier, *amount).await;
            assert_eq!(
                put.status,
                202,
                "case {label:?}: expected 202 allocating {amount}, got {} {}",
                put.status,
                put.text()
            );
            // The allocation is not instant, so the call that accepted it reports the
            // amount requested and nothing allocated yet.
            assert_eq!(
                reported(&put),
                expected(*amount, STATUS_IN_PROGRESS),
                "case {label:?}: PutProvisionedConcurrencyConfig must answer the \
                 configuration it stored, got {}",
                put.text()
            );
            assert!(
                put.string("/LastModified").is_some_and(|at| !at.is_empty()),
                "case {label:?}: the stored configuration must carry a LastModified, got {}",
                put.text()
            );

            // Read back after every call: a configuration that was answered but never
            // persisted fails here, and it is still being allocated.
            let pending = get_config(&router, &reference, qualifier).await;
            assert_eq!(
                pending.status,
                200,
                "case {label:?}: expected 200 from GetProvisionedConcurrencyConfig, got {} {}",
                pending.status,
                pending.text()
            );
            assert_eq!(
                reported(&pending),
                expected(*amount, STATUS_IN_PROGRESS),
                "case {label:?}: before the allocation is due the configuration reports \
                 nothing allocated, got {}",
                pending.text()
            );
        }

        let last = *case.puts.last().expect("every row allocates at least once");

        // The warm-up is owed by the CLOCK: once it is due, the whole requested amount is
        // allocated and available, and every reader agrees.
        make_allocation_due(&engine);
        let ready = get_config(&router, &reference, qualifier).await;
        assert_eq!(
            ready.status,
            200,
            "case {label:?}: expected 200 from GetProvisionedConcurrencyConfig, got {} {}",
            ready.status,
            ready.text()
        );
        assert_eq!(
            reported(&ready),
            expected(last, STATUS_READY),
            "case {label:?}: once the allocation is due the configuration is ready, got {}",
            ready.text()
        );

        // Reading it does not change what the next caller sees.
        let again = get_config(&router, &reference, qualifier).await;
        assert_eq!(
            reported(&again),
            reported(&ready),
            "case {label:?}: a second read must report the same configuration, got {}",
            again.text()
        );

        // The listing reports the same record, against the ARN of the alias or version it
        // belongs to, and it is reachable by the bare function name whichever form the
        // calls used.
        let list = list_configs(&router, function, "").await;
        assert_eq!(
            list.status,
            200,
            "case {label:?}: expected 200 from ListProvisionedConcurrencyConfigs, got {} {}",
            list.status,
            list.text()
        );
        let arn = qualified_arn(function, qualifier);
        assert_eq!(
            listed(&list, &arn),
            Some(expected(last, STATUS_READY)),
            "case {label:?}: the listing must report the configuration of {arn}, got {}",
            list.text()
        );

        // Deleting it removes exactly that configuration, and the qualifier holds none
        // again.
        let deleted = delete_config(&router, &reference, qualifier).await;
        assert_eq!(
            deleted.status,
            204,
            "case {label:?}: expected 204 from DeleteProvisionedConcurrencyConfig, got {} {}",
            deleted.status,
            deleted.text()
        );
        let gone = get_config(&router, &reference, qualifier).await;
        assert_eq!(
            gone.status,
            404,
            "case {label:?}: a deleted configuration is gone, got {} {}",
            gone.status,
            gone.text()
        );
        let empty = list_configs(&router, function, "").await;
        assert_eq!(
            listed(&empty, &arn),
            None,
            "case {label:?}: a deleted configuration must not be listed, got {}",
            empty.text()
        );
        // The operation is idempotent: the resource is gone either way.
        let repeat = delete_config(&router, &reference, qualifier).await;
        assert_eq!(
            repeat.status,
            204,
            "case {label:?}: deleting a configuration that is already gone must answer 204, \
             got {} {}",
            repeat.status,
            repeat.text()
        );
    }
}

#[tokio::test]
async fn each_qualifier_holds_its_own_configuration() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "provisioned-scoped";

    // Three qualifiers of one function: two published versions and an alias invoking the
    // first of them, each allocated a different amount, so no qualifier can report
    // another's — including the alias and the version it invokes, which are two
    // resources a configuration is added to separately.
    create_function_with_versions(&router, &engine, function, 2).await;
    create_alias(&router, function, "live", "1").await;
    let configured: &[(&str, i64)] = &[("1", 4), ("2", 7), ("live", 11)];
    for (qualifier, amount) in configured {
        let put = put_config(&router, function, qualifier, *amount).await;
        assert_eq!(
            put.status,
            202,
            "case {qualifier:?}: expected 202 allocating {amount}, got {} {}",
            put.status,
            put.text()
        );
    }
    make_allocation_due(&engine);

    for (qualifier, amount) in configured {
        let answer = get_config(&router, function, qualifier).await;
        assert_eq!(
            reported(&answer),
            expected(*amount, STATUS_READY),
            "case {qualifier:?}: each qualifier holds its own configuration, got {}",
            answer.text()
        );
    }

    // The listing reports every one of them, each against its own ARN.
    let list = list_configs(&router, function, "").await;
    for (qualifier, amount) in configured {
        let arn = qualified_arn(function, qualifier);
        assert_eq!(
            listed(&list, &arn),
            Some(expected(*amount, STATUS_READY)),
            "case {qualifier:?}: the listing must report {arn}, got {}",
            list.text()
        );
    }
    let count = list
        .json()
        .pointer("/ProvisionedConcurrencyConfigs")
        .and_then(Value::as_array)
        .map(Vec::len);
    assert_eq!(
        count,
        Some(configured.len()),
        "the listing must report exactly the {} configurations added, got {}",
        configured.len(),
        list.text()
    );

    // Deleting one leaves the others exactly as they were.
    let deleted = delete_config(&router, function, "2").await;
    assert_eq!(
        deleted.status,
        204,
        "deleting one configuration must answer 204, got {} {}",
        deleted.status,
        deleted.text()
    );
    for (qualifier, amount) in configured {
        let answer = get_config(&router, function, qualifier).await;
        if *qualifier == "2" {
            assert_eq!(
                answer.status,
                404,
                "case {qualifier:?}: the deleted configuration is gone, got {} {}",
                answer.status,
                answer.text()
            );
        } else {
            assert_eq!(
                reported(&answer),
                expected(*amount, STATUS_READY),
                "case {qualifier:?}: deleting another qualifier's configuration must leave \
                 this one alone, got {}",
                answer.text()
            );
        }
    }
}

#[tokio::test]
async fn a_listing_pages_through_the_configurations_in_qualifier_order() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "provisioned-paged";

    // Three configurations, so a page smaller than the set has to continue.
    create_function_with_versions(&router, &engine, function, 3).await;
    for qualifier in ["1", "2", "3"] {
        let put = put_config(&router, function, qualifier, 2).await;
        assert_eq!(
            put.status,
            202,
            "case {qualifier:?}: expected 202, got {} {}",
            put.status,
            put.text()
        );
    }

    let first = list_configs(&router, function, "&MaxItems=2").await;
    let arns = |answer: &Answer| -> Vec<String> {
        answer
            .json()
            .pointer("/ProvisionedConcurrencyConfigs")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry["FunctionArn"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    assert_eq!(
        arns(&first),
        vec![qualified_arn(function, "1"), qualified_arn(function, "2")],
        "a page of two must report the first two qualifiers in order, got {}",
        first.text()
    );
    let marker = first.string("/NextMarker");
    assert_eq!(
        marker.as_deref(),
        Some("2"),
        "a truncated page must carry the qualifier it ended on as NextMarker, got {}",
        first.text()
    );

    let second = list_configs(
        &router,
        function,
        &format!("&MaxItems=2&Marker={}", marker.unwrap_or_default()),
    )
    .await;
    assert_eq!(
        arns(&second),
        vec![qualified_arn(function, "3")],
        "the next page must continue after the marker, got {}",
        second.text()
    );
    assert_eq!(
        second.string("/NextMarker"),
        None,
        "the last page must carry no NextMarker, got {}",
        second.text()
    );
}

#[tokio::test]
async fn the_unpublished_version_holds_no_provisioned_concurrency() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "provisioned-latest";
    create_function_with_versions(&router, &engine, function, 1).await;

    // Every documented way of naming the unpublished version, on every operation that
    // takes a qualifier: a configuration "is added to a function's alias or version", and
    // `$LATEST` is neither, so none of them can address one there.
    let references = [
        function.to_string(),
        format!("function:{function}"),
        format!("{ACCOUNT}:function:{function}"),
        format!("{function}:{VERSION_LATEST}"),
    ];
    for reference in &references {
        for answer in [
            put_config(&router, reference, VERSION_LATEST, 5).await,
            get_config(&router, reference, VERSION_LATEST).await,
            delete_config(&router, reference, VERSION_LATEST).await,
        ] {
            assert_eq!(
                answer.status,
                400,
                "case {reference:?}: expected 400 for the unpublished version, got {} {}",
                answer.status,
                answer.text()
            );
            assert!(
                answer
                    .error_code()
                    .contains("InvalidParameterValueException"),
                "case {reference:?}: expected InvalidParameterValueException, got {:?}",
                answer.error_code()
            );
        }
    }

    // None of those calls stored anything, on `$LATEST` or anywhere else.
    let list = list_configs(&router, function, "").await;
    let count = list
        .json()
        .pointer("/ProvisionedConcurrencyConfigs")
        .and_then(Value::as_array)
        .map(Vec::len);
    assert_eq!(
        count,
        Some(0),
        "a refused call must store nothing, got {}",
        list.text()
    );
}

#[tokio::test]
async fn a_name_or_qualifier_that_names_no_version_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "provisioned-present";
    create_function_with_versions(&router, &engine, function, 1).await;
    let put = put_config(&router, function, "1", 6).await;
    assert_eq!(
        put.status,
        202,
        "the configuration under test must be stored, got {} {}",
        put.status,
        put.text()
    );

    // A function that does not exist, in each documented `FunctionName` form.
    let missing = [
        "provisioned-absent".to_string(),
        format!("{ACCOUNT}:function:provisioned-absent"),
        format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:provisioned-absent"),
    ];
    for reference in &missing {
        for answer in [
            put_config(&router, reference, "1", 1).await,
            get_config(&router, reference, "1").await,
            delete_config(&router, reference, "1").await,
            list_configs(&router, reference, "").await,
        ] {
            assert_eq!(
                answer.status,
                404,
                "case {reference:?}: expected 404 for a function that does not exist, got {} {}",
                answer.status,
                answer.text()
            );
            assert!(
                answer.error_code().contains("ResourceNotFoundException"),
                "case {reference:?}: expected ResourceNotFoundException, got {:?}",
                answer.error_code()
            );
        }
    }

    // A qualifier of an existing function that names no version and no alias: a published
    // version the function never published, and an alias-shaped name nobody created.
    for qualifier in ["7", "staging"] {
        for answer in [
            put_config(&router, function, qualifier, 1).await,
            get_config(&router, function, qualifier).await,
            delete_config(&router, function, qualifier).await,
        ] {
            assert_eq!(
                answer.status,
                404,
                "case {qualifier:?}: expected 404 for a qualifier naming no version, got {} {}",
                answer.status,
                answer.text()
            );
            assert!(
                answer.error_code().contains("ResourceNotFoundException"),
                "case {qualifier:?}: expected ResourceNotFoundException, got {:?}",
                answer.error_code()
            );
        }
    }

    // A `FunctionName` whose own qualifier and the `Qualifier` parameter name two
    // different versions names no single resource.
    let mismatched = put_config(&router, &format!("{function}:1"), "live", 1).await;
    assert_eq!(
        mismatched.status,
        400,
        "expected 400 when the name and the Qualifier disagree, got {} {}",
        mismatched.status,
        mismatched.text()
    );

    // None of the refusals changed the configuration the function does hold.
    make_allocation_due(&engine);
    let stored = get_config(&router, function, "1").await;
    assert_eq!(
        reported(&stored),
        expected(6, STATUS_READY),
        "a refused call must leave the stored configuration alone, got {}",
        stored.text()
    );
}

/// One row of the quota table: what the function reserves, what another function has
/// already allocated, the amount this call asks for, and whether the documented rules
/// admit it.
struct QuotaCase {
    name: &'static str,
    /// What `PutFunctionConcurrency` reserves for the function under test.
    reserved: Option<i64>,
    /// What a second function allocates on a version of its own before the call.
    allocated_elsewhere: Option<i64>,
    requested: i64,
    admitted: bool,
}

/// The rows: each published bound of the two documented rules, and one past it.
const QUOTA_CASES: &[QuotaCase] = &[
    QuotaCase {
        name: "the whole allocatable capacity, leaving exactly the minimum unreserved",
        reserved: None,
        allocated_elsewhere: None,
        requested: MAX_ALLOCATABLE,
        admitted: true,
    },
    QuotaCase {
        name: "one past the allocatable capacity",
        reserved: None,
        allocated_elsewhere: None,
        requested: MAX_ALLOCATABLE + 1,
        admitted: false,
    },
    QuotaCase {
        name: "the whole account concurrency limit",
        reserved: None,
        allocated_elsewhere: None,
        requested: CONCURRENT_EXECUTIONS_LIMIT,
        admitted: false,
    },
    QuotaCase {
        name: "all that is left beside another function's allocation",
        reserved: None,
        allocated_elsewhere: Some(400),
        requested: MAX_ALLOCATABLE - 400,
        admitted: true,
    },
    QuotaCase {
        name: "one past what is left beside another function's allocation",
        reserved: None,
        allocated_elsewhere: Some(400),
        requested: MAX_ALLOCATABLE - 400 + 1,
        admitted: false,
    },
    QuotaCase {
        name: "all of the function's own reserved concurrency",
        reserved: Some(50),
        allocated_elsewhere: None,
        requested: 50,
        admitted: true,
    },
    QuotaCase {
        name: "one past the function's own reserved concurrency",
        reserved: Some(50),
        allocated_elsewhere: None,
        requested: 51,
        admitted: false,
    },
];

#[tokio::test]
async fn an_allocation_must_stay_inside_the_documented_quotas() {
    for case in QUOTA_CASES {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let label = case.name;
        let function = "provisioned-quota";

        create_function_with_versions(&router, &engine, function, 1).await;
        if let Some(reserved) = case.reserved {
            put_reserved_concurrency(&router, function, reserved).await;
        }
        if let Some(elsewhere) = case.allocated_elsewhere {
            create_function_with_versions(&router, &engine, "provisioned-neighbour", 1).await;
            let put = put_config(&router, "provisioned-neighbour", "1", elsewhere).await;
            assert_eq!(
                put.status,
                202,
                "case {label:?}: the neighbour's allocation of {elsewhere} must succeed, got \
                 {} {}",
                put.status,
                put.text()
            );
        }

        let put = put_config(&router, function, "1", case.requested).await;
        if case.admitted {
            assert_eq!(
                put.status,
                202,
                "case {label:?}: expected 202 allocating {}, got {} {}",
                case.requested,
                put.status,
                put.text()
            );
            make_allocation_due(&engine);
            let stored = get_config(&router, function, "1").await;
            assert_eq!(
                reported(&stored),
                expected(case.requested, STATUS_READY),
                "case {label:?}: an admitted allocation must be stored, got {}",
                stored.text()
            );
        } else {
            assert_eq!(
                put.status,
                400,
                "case {label:?}: expected 400 allocating {}, got {} {}",
                case.requested,
                put.status,
                put.text()
            );
            assert!(
                put.error_code().contains("InvalidParameterValueException"),
                "case {label:?}: expected InvalidParameterValueException, got {:?}",
                put.error_code()
            );
            // A refused call stores nothing.
            let stored = get_config(&router, function, "1").await;
            assert_eq!(
                stored.status,
                404,
                "case {label:?}: a refused allocation must store nothing, got {} {}",
                stored.status,
                stored.text()
            );
        }
    }
}
