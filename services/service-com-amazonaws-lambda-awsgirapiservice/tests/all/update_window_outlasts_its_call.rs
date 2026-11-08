// SPDX-License-Identifier: CC0-1.0

//! The in-flight window an accepted code update opens, as a CALLER sees it.
//!
//! The rule under test: an accepted update of a function opens a window in which the
//! function's `LastUpdateStatus` is `InProgress` — "When you update a function, Lambda
//! provisions an instance of the function and its supporting resources. [...] During this
//! time, you can't modify the function, but you can still invoke it" — and every call that
//! would modify the function while it is open is refused with the modelled
//! `ResourceConflictException` (client, HTTP 409) naming the function's stored ARN, with no
//! effect on the configuration. The window is a span of the CLOCK, so it has to outlast the
//! call that opened it: a caller whose very next request reads the function back, or
//! modifies it again, meets it — including the caller that uploaded a multi-megabyte
//! deployment package, which is the normal size of one.
//!
//! What makes this suite different from `function_update_window_conflict`, which pins the
//! same refusal by moving the stored due instant: here NOTHING is injected and nothing is
//! waited for before the next call, so an emulator whose window closes inside the upload it
//! was opened by fails. The rows differ along the one thing the window has to survive — the
//! size of the package the update carried — from a few bytes to the megabyte scale, and each
//! row also pins the window CLOSING on its own, so the conflict is a window and not a
//! permanent refusal.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{ACTIVATION_DELAY, UPDATE_DELAY};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`), which is what
/// routes a request here and what Region the minted ARNs carry. Nothing in it is verified:
/// the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The Region the scope above names.
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

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(200);

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    /// The response body as JSON, or `Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The response body as text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The wire error code (`__type`) of an error response.
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .map_or_else(|| self.text(), str::to_string)
    }

    /// The `message` of an error response.
    fn error_message(&self) -> String {
        let body = self.json();
        body.get("message")
            .or_else(|| body.get("Message"))
            .and_then(Value::as_str)
            .map_or_else(|| self.text(), str::to_string)
    }

    /// One string member of a success body.
    fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request, the shape every call here uses.
async fn json_call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let host = format!("lambda.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://{host}{uri}"))
        .header("host", host)
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

/// Deployment package bytes of a given length. The content is irrelevant to the API — only
/// its length and its digest are reported — but it differs per length so two packages never
/// share a digest.
fn package_bytes(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).unwrap_or(0))
        .collect()
}

/// A `ZipFile` member: restJson1 carries a blob base64-encoded in a JSON body.
fn zip_file(bytes: &[u8]) -> Value {
    Value::String(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// The unqualified ARN a function of `name` carries.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// Lambda's sentence for a modification refused because an update of the function is still
/// being applied, which `crate::state::update_in_progress_message` states once for this
/// service.
fn conflict_message(name: &str) -> String {
    format!(
        "The operation cannot be performed at this time. An update is in progress for \
         resource: {}",
        function_arn(name)
    )
}

/// The `LastUpdateStatus` and `Timeout` `GetFunctionConfiguration` reports.
async fn read_back(router: &MultiServiceRouter, name: &str) -> (Option<String>, Option<i64>) {
    let answer = json_call(
        router,
        "GET",
        &format!("{FUNCTIONS_URI}/{name}/configuration"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {name:?}: expected 200 from GetFunctionConfiguration, got {} {}",
        answer.status,
        answer.text()
    );
    (
        answer.string("LastUpdateStatus"),
        answer.json().get("Timeout").and_then(Value::as_i64),
    )
}

/// RULE: the window an accepted code update opens outlasts the call that opened it, so the
/// caller's next read sees `InProgress` and its next modification is refused 409 — whatever
/// size the package was.
#[tokio::test]
async fn the_window_an_accepted_code_update_opens_outlasts_the_call_that_opened_it() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let name = "deployed";
    let created = json_call(
        &router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Timeout": 3,
            "Code": { "ZipFile": zip_file(&package_bytes(64)) },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "the function under test must be created: got {} {}",
        created.status,
        created.text()
    );
    // "During this time, you can't invoke or modify the function": the move to `Active` is
    // owed by the clock, and this waits past the instant the create made it due.
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;

    // One row per package SIZE, which is what the window has to survive: the bytes of a
    // real deployment package travel base64-encoded in the request body, so a megabyte-scale
    // package is a megabyte-scale request. Each row also asks for its own `Timeout`, so the
    // value a refused update would have written is a value no other row writes.
    let rows: &[(&str, usize, i64)] = &[
        ("a few bytes", 64, 11),
        ("a megabyte", 1024 * 1024, 12),
        ("several megabytes", 8 * 1024 * 1024, 13),
    ];

    for (case, package_size, refused_timeout) in rows {
        let (_, settled_timeout) = read_back(&router, name).await;

        let updated = json_call(
            &router,
            "PUT",
            &format!("{FUNCTIONS_URI}/{name}/code"),
            &json!({ "ZipFile": zip_file(&package_bytes(*package_size)) }),
        )
        .await;
        assert_eq!(
            updated.status,
            200,
            "case {case:?}: expected 200 from UpdateFunctionCode, got {} {}",
            updated.status,
            updated.text()
        );
        let answered = updated.string("LastUpdateStatus");
        assert_eq!(
            answered.as_deref(),
            Some(IN_PROGRESS),
            "case {case:?}: expected the update to answer LastUpdateStatus {IN_PROGRESS}, got \
             {answered:?}"
        );

        // The caller's NEXT call, with nothing waited for in between.
        let (status, timeout) = read_back(&router, name).await;
        assert_eq!(
            status.as_deref(),
            Some(IN_PROGRESS),
            "case {case:?}: expected the read right after the update to report \
             LastUpdateStatus {IN_PROGRESS}, got {status:?}"
        );
        assert_eq!(
            timeout, settled_timeout,
            "case {case:?}: expected the update to leave Timeout {settled_timeout:?}, got \
             {timeout:?}"
        );

        // And a modification issued inside that window is refused, with no effect.
        let refused = json_call(
            &router,
            "PUT",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            &json!({ "Timeout": refused_timeout }),
        )
        .await;
        assert_eq!(
            refused.status,
            409,
            "case {case:?}: expected 409 from UpdateFunctionConfiguration inside the window, \
             got {} {}",
            refused.status,
            refused.text()
        );
        let code = refused.error_code();
        assert!(
            code.contains("ResourceConflictException"),
            "case {case:?}: expected the ResourceConflictException error code, got {code:?}"
        );
        let expected = conflict_message(name);
        let message = refused.error_message();
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
        let (_, unchanged) = read_back(&router, name).await;
        assert_eq!(
            unchanged, settled_timeout,
            "case {case:?}: expected the refused update to leave Timeout {settled_timeout:?}, \
             got {unchanged:?}"
        );

        // The window is a window: the move to `Successful` is owed by the clock, and once it
        // is due the same modification is accepted.
        tokio::time::sleep(UPDATE_DELAY + SLACK).await;
        let (settled, _) = read_back(&router, name).await;
        assert_eq!(
            settled.as_deref(),
            Some(SUCCESSFUL),
            "case {case:?}: expected LastUpdateStatus {SUCCESSFUL} once the window closed, got \
             {settled:?}"
        );
        let accepted = json_call(
            &router,
            "PUT",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            &json!({ "Timeout": refused_timeout }),
        )
        .await;
        assert_eq!(
            accepted.status,
            200,
            "case {case:?}: expected 200 from UpdateFunctionConfiguration once the window \
             closed, got {} {}",
            accepted.status,
            accepted.text()
        );
        // That accepted update opens a window of its own, which the next row waits out.
        tokio::time::sleep(UPDATE_DELAY + SLACK).await;
        let (settled, applied) = read_back(&router, name).await;
        assert_eq!(
            applied,
            Some(*refused_timeout),
            "case {case:?}: expected the accepted update to store Timeout {refused_timeout}, \
             got {applied:?}"
        );
        assert_eq!(
            settled.as_deref(),
            Some(SUCCESSFUL),
            "case {case:?}: expected LastUpdateStatus {SUCCESSFUL} after the second window, got \
             {settled:?}"
        );
    }
}
