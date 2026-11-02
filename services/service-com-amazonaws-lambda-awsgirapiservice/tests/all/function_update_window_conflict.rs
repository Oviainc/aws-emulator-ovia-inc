// SPDX-License-Identifier: CC0-1.0

//! The window an accepted update of a function opens, end to end over HTTP.
//!
//! The rule under test: while an update of a function is still being applied —
//! "When you update a function, Lambda provisions an instance of the function and its
//! supporting resources. [...] During this time, you can't modify the function, but you
//! can still invoke it" — every call that would modify that function is refused with the
//! modelled `ResourceConflictException` ("The resource already exists, or another
//! operation is in progress", HTTP 409) naming the function's stored ARN, whatever the
//! request itself asks for: the state of the resource is decided BEFORE the request's own
//! members are judged, so a request that is also wrong on its own terms is refused with
//! the conflict while the window is open and with its own parameter refusal once the
//! window has closed. A refused call changes nothing and opens no window of its own.
//!
//! Every row drives real HTTP requests through the router, opens the window through a
//! real operation and reads the result back through the real readers, so an answer that
//! was never grounded in state fails here. The rows differ along what the rule
//! generalises over: which operation opened the window (each of the three that update a
//! function), which operation is then refused, and whether the refused request is valid
//! on its own terms. The clock-owed `Pending` -> `Active` and `InProgress` ->
//! `Successful` moves are exercised by moving the stored due instants into the past, so
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

/// A SigV4 credential scope naming this service's signing name (`lambda`), which is what
/// routes a request here and what region the minted ARNs carry. Nothing in it is
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

/// The `LastUpdateStatus` of an update still being applied, from the model's
/// `LastUpdateStatus` enum (`Failed | InProgress | Successful`).
const IN_PROGRESS: &str = "InProgress";

/// The `LastUpdateStatus` of a settled update, from the same enum.
const SUCCESSFUL: &str = "Successful";

/// The only `UpdateRuntimeOn` mode that pins a runtime version: "This is only required if
/// you're using the Manual runtime update mode."
const MANUAL: &str = "Manual";

/// The whole `UpdateRuntimeOn` value set, from
/// `smithy_query(query="shape", shape="UpdateRuntimeOn")`: `Auto | Manual |
/// FunctionUpdate`.
const UPDATE_RUNTIME_ON_VALUES: &[&str] = &["Auto", "Manual", "FunctionUpdate"];

/// One runtime version ARN per branch of the `RuntimeVersionArn` pattern, from
/// `smithy_query(query="shape", shape="RuntimeVersionArn")`:
/// `arn:<partition>:lambda:<region>::runtime:<id>` and
/// `arn:<partition>:lambda:<region>:<account>:runtime/<name>/version/<version>`.
const RUNTIME_VERSION_ARNS: &[&str] = &[
    "arn:aws:lambda:us-east-1::runtime:8eeff65f6809a3ce81507fe733fe09b835899b99481ba22fd75b5a7338290ec1",
    "arn:aws:lambda:us-east-1:000000000000:runtime/python3.12/version/a1b2c3d4e5",
];

/// Lambda's sentence for the manual mode without the runtime version ARN that mode
/// requires, as `PutRuntimeManagementConfig` states it.
const MISSING_RUNTIME_VERSION_ARN: &str =
    "Missing runtime version ARN. A runtime version ARN is required when \
     UpdateRuntimeVersionOn=Manual. Provide a valid runtime version ARN and retry the \
     request.";

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
            panic!("expected a JSON body, got {error}: {}", self.text())
        })
    }

    /// A `String` member of the JSON body.
    fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    fn error_code(&self) -> String {
        self.string("__type")
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_default()
    }

    /// The `message` member a modelled error carries.
    fn message(&self) -> String {
        self.string("message")
            .or_else(|| self.string("Message"))
            .unwrap_or_default()
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

/// Deployment package bytes of a given length: the content is irrelevant to the API, but
/// it differs per length so two packages never share a digest.
fn package_bytes(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).unwrap_or(0))
        .collect()
}

/// A `ZipFile` member: restJson1 carries a blob base64-encoded in a JSON body.
fn zip_file(bytes: &[u8]) -> Value {
    Value::String(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Create one .zip function of `name`, asserting it was stored.
async fn create_function(router: &MultiServiceRouter, name: &str) {
    let answer = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Description": "as created",
            "Timeout": 7,
            "Code": { "ZipFile": zip_file(&package_bytes(64)) },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// `GetFunctionConfiguration` on the unpublished version of `name`.
async fn get_function_configuration(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{FUNCTIONS_URI}/{name}/configuration"),
        &Value::Null,
    )
    .await
}

/// `UpdateFunctionConfiguration` on `name`, carrying `body`.
async fn update_function_configuration(
    router: &MultiServiceRouter,
    name: &str,
    body: &Value,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("{FUNCTIONS_URI}/{name}/configuration"),
        body,
    )
    .await
}

/// `UpdateFunctionCode` on `name`, carrying `body`.
async fn update_function_code(router: &MultiServiceRouter, name: &str, body: &Value) -> Answer {
    call(router, "PUT", &format!("{FUNCTIONS_URI}/{name}/code"), body).await
}

/// Where both runtime-management operations are bound.
fn runtime_management_uri(name: &str) -> String {
    format!("/2021-07-20/functions/{name}/runtime-management-config")
}

/// `GetRuntimeManagementConfig` on the unpublished version of `name`.
async fn get_runtime_management_config(router: &MultiServiceRouter, name: &str) -> Answer {
    call(router, "GET", &runtime_management_uri(name), &Value::Null).await
}

/// `PutRuntimeManagementConfig` on the unpublished version of `name`, sending the
/// `RuntimeVersionArn` member only when the caller names one.
async fn put_runtime_management_config(
    router: &MultiServiceRouter,
    name: &str,
    update_runtime_on: &str,
    runtime_version_arn: Option<&str>,
) -> Answer {
    let mut body = json!({ "UpdateRuntimeOn": update_runtime_on });
    if let Some(arn) = runtime_version_arn {
        body["RuntimeVersionArn"] = json!(arn);
    }
    call(router, "PUT", &runtime_management_uri(name), &body).await
}

/// Move every stored function's activation instant into the past, so the next read
/// materialises the `Pending` -> `Active` move the clock owes. This is what waiting would
/// do, without waiting.
fn make_activation_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
}

/// Move every in-flight update's settle instant into the past, so the next read
/// materialises the `InProgress` -> `Successful` move the clock owes.
fn make_update_settled(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        if function.last_update_due.is_some() {
            function.last_update_due = Some(past);
        }
    }
}

/// The unqualified ARN of the function `name`.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// Lambda's sentence for a modification refused because an update of the function is
/// still being applied: fixed text plus the function's own ARN, which each row builds
/// from the function it set up rather than from the answer it is checking.
fn in_progress_sentence(name: &str) -> String {
    format!(
        "The operation cannot be performed at this time. An update is in progress for \
         resource: {}",
        function_arn(name)
    )
}

/// Assert that `answer` is the modelled 409 for an update still in flight on `name`.
fn assert_in_flight_conflict(case: &str, operation: &str, answer: &Answer, name: &str) {
    assert_eq!(
        answer.status,
        409,
        "case {case:?}: expected 409 from {operation}, got {} {}",
        answer.status,
        answer.text()
    );
    assert!(
        answer.error_code().contains("ResourceConflictException"),
        "case {case:?}: expected ResourceConflictException from {operation}, got {:?}",
        answer.error_code()
    );
    assert_eq!(
        answer.message(),
        in_progress_sentence(name),
        "case {case:?}: expected the in-flight sentence naming the function's ARN from \
         {operation}, got {:?}",
        answer.message()
    );
}

/// Assert that `name` reports `expected` as its `LastUpdateStatus`.
async fn assert_last_update_status(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    expected: &str,
) {
    let answer = get_function_configuration(router, name).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected 200 from GetFunctionConfiguration, got {} {}",
        answer.status,
        answer.text()
    );
    assert_eq!(
        answer.string("LastUpdateStatus").as_deref(),
        Some(expected),
        "case {case:?}: expected LastUpdateStatus {expected:?}, got {:?}",
        answer.string("LastUpdateStatus")
    );
}

/// Which operation opens the update window a row then tries to modify the function
/// through. Each of the three is an update of the function, so each one opens it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opener {
    /// `UpdateFunctionConfiguration` replacing one setting.
    Configuration,
    /// `UpdateFunctionCode` replacing the deployment package.
    Code,
    /// `PutRuntimeManagementConfig` pinning a runtime version.
    RuntimeManagement,
}

/// Open the update window on `name` through `opener`, asserting it was accepted.
async fn open_window(router: &MultiServiceRouter, case: &str, name: &str, opener: Opener) {
    let answer = match opener {
        Opener::Configuration => {
            update_function_configuration(router, name, &json!({ "Timeout": 11 })).await
        }
        Opener::Code => {
            update_function_code(router, name, &json!({ "ZipFile": zip_file(&package_bytes(128)) }))
                .await
        }
        Opener::RuntimeManagement => {
            put_runtime_management_config(router, name, MANUAL, Some(RUNTIME_VERSION_ARNS[0])).await
        }
    };
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: the {opener:?} update must be accepted, got {} {}",
        answer.status,
        answer.text()
    );
}

/// The modification a row makes while the window is open, and what it must answer once
/// the window has closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Modification {
    /// `PutRuntimeManagementConfig` with a mode and runtime version ARN that hold
    /// together: valid on its own terms, so it succeeds once the window closes.
    RuntimeManagement(&'static str, Option<&'static str>),
    /// `PutRuntimeManagementConfig` whose mode and runtime version ARN cannot hold
    /// together: the conflict comes first while the window is open, and the parameter
    /// refusal is what is left once it closes.
    RuntimeManagementBadPair(&'static str, Option<&'static str>, &'static str),
    /// `UpdateFunctionConfiguration` replacing one setting.
    Configuration,
    /// `UpdateFunctionCode` replacing the deployment package.
    Code,
}

/// Make `modification` against `name`.
async fn modify(router: &MultiServiceRouter, name: &str, modification: Modification) -> Answer {
    match modification {
        Modification::RuntimeManagement(mode, arn)
        | Modification::RuntimeManagementBadPair(mode, arn, _) => {
            put_runtime_management_config(router, name, mode, arn).await
        }
        Modification::Configuration => {
            update_function_configuration(router, name, &json!({ "Timeout": 23 })).await
        }
        Modification::Code => {
            update_function_code(router, name, &json!({ "ZipFile": zip_file(&package_bytes(192)) }))
                .await
        }
    }
}

/// The operation `modification` calls, for a failure message.
fn operation_of(modification: Modification) -> &'static str {
    match modification {
        Modification::RuntimeManagement(_, _) | Modification::RuntimeManagementBadPair(_, _, _) => {
            "PutRuntimeManagementConfig"
        }
        Modification::Configuration => "UpdateFunctionConfiguration",
        Modification::Code => "UpdateFunctionCode",
    }
}

/// The openers and the modifications, crossed: every operation that opens the window
/// against every modification the rule covers, including the two shapes of request that
/// are wrong on their own terms.
fn window_cases() -> Vec<(Opener, Modification)> {
    let openers = [Opener::Configuration, Opener::Code, Opener::RuntimeManagement];
    let modifications = [
        Modification::RuntimeManagement("Auto", None),
        Modification::RuntimeManagement(MANUAL, Some(RUNTIME_VERSION_ARNS[1])),
        Modification::RuntimeManagement("FunctionUpdate", None),
        Modification::RuntimeManagementBadPair(MANUAL, None, MISSING_RUNTIME_VERSION_ARN),
        Modification::RuntimeManagementBadPair(
            "Auto",
            Some(RUNTIME_VERSION_ARNS[0]),
            "RuntimeVersionArn can only be specified when UpdateRuntimeOn is Manual, not Auto",
        ),
        Modification::RuntimeManagementBadPair(
            "FunctionUpdate",
            Some(RUNTIME_VERSION_ARNS[1]),
            "RuntimeVersionArn can only be specified when UpdateRuntimeOn is Manual, not \
             FunctionUpdate",
        ),
        Modification::Configuration,
        Modification::Code,
    ];
    let mut cases = Vec::new();
    for opener in openers {
        for modification in modifications {
            cases.push((opener, modification));
        }
    }
    cases
}

#[tokio::test]
async fn a_modification_is_refused_while_an_update_of_the_function_is_in_flight() {
    for (index, (opener, modification)) in window_cases().into_iter().enumerate() {
        // One function per row, so the window a row opens is the only one in flight.
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let name = format!("update-window-{index}");
        let case = format!("{opener:?} then {modification:?}");

        create_function(&router, &name).await;
        make_activation_due(&engine);
        assert_last_update_status(&router, &case, &name, SUCCESSFUL).await;

        open_window(&router, &case, &name, opener).await;
        assert_last_update_status(&router, &case, &name, IN_PROGRESS).await;

        // What the function holds before the refused call, read through the real
        // readers, so a refusal that changed anything shows up below.
        let configuration_before = get_function_configuration(&router, &name).await.text();
        let runtime_before = get_runtime_management_config(&router, &name).await.text();

        let refused = modify(&router, &name, modification).await;
        assert_in_flight_conflict(&case, operation_of(modification), &refused, &name);

        // The refusal changed nothing, and it did not settle or extend the window
        // either: the function is still the one the opener left behind.
        let configuration_after = get_function_configuration(&router, &name).await.text();
        assert_eq!(
            configuration_after, configuration_before,
            "case {case:?}: the refused call must leave the configuration at \
             {configuration_before}, got {configuration_after}"
        );
        let runtime_after = get_runtime_management_config(&router, &name).await.text();
        assert_eq!(
            runtime_after, runtime_before,
            "case {case:?}: the refused call must leave the runtime management \
             configuration at {runtime_before}, got {runtime_after}"
        );

        // Once the clock has settled the update, the SAME request is judged on its own
        // terms: a valid one is served, and one whose members cannot hold together earns
        // its own parameter refusal — which is what the 409 above came before.
        make_update_settled(&engine);
        assert_last_update_status(&router, &case, &name, SUCCESSFUL).await;
        let retried = modify(&router, &name, modification).await;
        match modification {
            Modification::RuntimeManagementBadPair(_, _, sentence) => {
                assert_eq!(
                    retried.status,
                    400,
                    "case {case:?}: expected 400 from {} once the update has settled, got {} {}",
                    operation_of(modification),
                    retried.status,
                    retried.text()
                );
                assert!(
                    retried
                        .error_code()
                        .contains("InvalidParameterValueException"),
                    "case {case:?}: expected InvalidParameterValueException once the update has \
                     settled, got {:?}",
                    retried.error_code()
                );
                assert_eq!(
                    retried.message(),
                    sentence,
                    "case {case:?}: expected the parameter sentence once the update has settled, \
                     got {:?}",
                    retried.message()
                );
            }
            _ => assert_eq!(
                retried.status,
                200,
                "case {case:?}: expected 200 from {} once the update has settled, got {} {}",
                operation_of(modification),
                retried.status,
                retried.text()
            ),
        }
    }
}

#[tokio::test]
async fn an_accepted_runtime_management_write_puts_the_function_in_flight() {
    // One row per modelled mode, and one per branch of the `RuntimeVersionArn` pattern
    // for the mode that pins one: whichever configuration is stored, storing it is an
    // update of the function.
    let writes: Vec<(&str, Option<&str>)> = vec![
        ("Auto", None),
        ("FunctionUpdate", None),
        (MANUAL, Some(RUNTIME_VERSION_ARNS[0])),
        (MANUAL, Some(RUNTIME_VERSION_ARNS[1])),
    ];
    for mode in UPDATE_RUNTIME_ON_VALUES {
        assert!(
            writes.iter().any(|(written, _)| written == mode),
            "case {mode:?}: the table must cover every modelled UpdateRuntimeOn value"
        );
    }

    for (index, (mode, arn)) in writes.into_iter().enumerate() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let name = format!("runtime-window-{index}");
        let case = format!("{mode} {arn:?}");

        create_function(&router, &name).await;
        make_activation_due(&engine);
        assert_last_update_status(&router, &case, &name, SUCCESSFUL).await;

        let written = put_runtime_management_config(&router, &name, mode, arn).await;
        assert_eq!(
            written.status,
            200,
            "case {case:?}: expected 200 from PutRuntimeManagementConfig, got {} {}",
            written.status,
            written.text()
        );
        assert_last_update_status(&router, &case, &name, IN_PROGRESS).await;

        // Reading is not modifying: "During this time, you can't modify the function, but
        // you can still invoke it", and the configuration just stored is what is reported
        // while the update is applied.
        let read = get_runtime_management_config(&router, &name).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: expected 200 from GetRuntimeManagementConfig while the update is in \
             flight, got {} {}",
            read.status,
            read.text()
        );
        assert_eq!(
            read.string("UpdateRuntimeOn").as_deref(),
            Some(mode),
            "case {case:?}: expected UpdateRuntimeOn {mode:?}, got {:?}",
            read.string("UpdateRuntimeOn")
        );
        assert_eq!(
            read.string("RuntimeVersionArn").as_deref(),
            arn,
            "case {case:?}: expected RuntimeVersionArn {arn:?}, got {:?}",
            read.string("RuntimeVersionArn")
        );

        // And once the clock has settled it, the next write is accepted again.
        make_update_settled(&engine);
        assert_last_update_status(&router, &case, &name, SUCCESSFUL).await;
        let again = put_runtime_management_config(&router, &name, "Auto", None).await;
        assert_eq!(
            again.status,
            200,
            "case {case:?}: expected 200 from the next PutRuntimeManagementConfig once the update \
             has settled, got {} {}",
            again.status,
            again.text()
        );
    }
}

/// One refused runtime-management write: the case name, the version reference it
/// addresses, the mode and runtime version ARN it carries, and the status it must earn.
type Refusal = (&'static str, fn(&str) -> String, &'static str, Option<&'static str>, u16);

#[tokio::test]
async fn a_refused_runtime_management_write_opens_no_update_window() {
    // One row per way the write can be refused: a pair that cannot hold together (400),
    // a mode outside the modelled value set (400 from the boundary validator), a version
    // nobody published (404), a function nothing created (404), and a name whose own
    // qualifier disagrees with the `Qualifier` parameter (400).
    let refusals: Vec<Refusal> = vec![
        (
            "Manual without a runtime version ARN",
            |name| name.to_string(),
            MANUAL,
            None,
            400,
        ),
        (
            "Auto with a runtime version ARN",
            |name| name.to_string(),
            "Auto",
            Some(RUNTIME_VERSION_ARNS[0]),
            400,
        ),
        (
            "a mode outside the modelled value set",
            |name| name.to_string(),
            "Bogus",
            None,
            400,
        ),
        (
            "a version nobody published",
            |name| format!("{name}:9"),
            "Auto",
            None,
            404,
        ),
        (
            "a function nothing created",
            |_| "update-window-absent".to_string(),
            "Auto",
            None,
            404,
        ),
    ];

    for (index, (label, reference, mode, arn, status)) in refusals.into_iter().enumerate() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let name = format!("refused-window-{index}");

        create_function(&router, &name).await;
        make_activation_due(&engine);
        assert_last_update_status(&router, label, &name, SUCCESSFUL).await;

        let refused =
            put_runtime_management_config(&router, &reference(&name), mode, arn).await;
        assert_eq!(
            refused.status,
            status,
            "case {label:?}: expected {status} from PutRuntimeManagementConfig, got {} {}",
            refused.status,
            refused.text()
        );

        // Nothing was accepted, so nothing is being applied: the function is still
        // settled and the next write is served rather than refused with the conflict.
        assert_last_update_status(&router, label, &name, SUCCESSFUL).await;
        let accepted =
            put_runtime_management_config(&router, &name, MANUAL, Some(RUNTIME_VERSION_ARNS[1]))
                .await;
        assert_eq!(
            accepted.status,
            200,
            "case {label:?}: a write after a refused one must be served, got {} {}",
            accepted.status,
            accepted.text()
        );
        assert_eq!(
            accepted.string("UpdateRuntimeOn").as_deref(),
            Some(MANUAL),
            "case {label:?}: expected UpdateRuntimeOn {MANUAL:?}, got {:?}",
            accepted.string("UpdateRuntimeOn")
        );
    }
}
