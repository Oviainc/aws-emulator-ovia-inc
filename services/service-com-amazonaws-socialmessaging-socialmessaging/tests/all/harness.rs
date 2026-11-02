// SPDX-License-Identifier: CC0-1.0

//! What every suite in this crate needs to drive the emulator: one emulator behind the
//! real router dispatch, and the HTTP call that reaches it.
//!
//! Factored out of the per-suite helpers so the suites that assert WHAT A MESSAGE SAYS
//! (`validation_message_rendering`, `not_found_message`) and the one that drives the
//! Conversions API (`conversion_events`) share one request path: a divergence between two
//! copies of the harness would read as a divergence in the service.

use bytes::Bytes;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use service_com_amazonaws_socialmessaging_socialmessaging::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A SigV4 `Authorization` header scoped to this service, which is what the router
/// routes on (`emulator_server::resolve_route_key`) and what the handlers read the
/// ARN region from.
const SIGNED: &str = "AWS4-HMAC-SHA256 \
                      Credential=AKIDEXAMPLE/20260101/us-east-1/social-messaging/aws4_request, \
                      SignedHeaders=host, Signature=0";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub const ACCOUNT_ID: &str = "000000000000";

/// The header the console's sign-up flow presents (`CONSOLE_CSRF_HEADER` in
/// `src/state.rs`), and a token value for it. `AssociateWhatsAppBusinessAccount` is served
/// through that flow alone, so every call that drives a sign-up goes through
/// [`console_call`]; a suite asserting the refusal calls [`call`], which presents nothing.
pub const CONSOLE_CSRF_HEADER: &str = "x-csrf-token";
/// A console CSRF token: any non-empty value is a request that came through the flow.
pub const CONSOLE_CSRF_TOKEN: &str = "console-csrf-token";

/// One emulator: a single shared engine behind the service, mounted in the router.
pub fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator, with the shared engine beside it — for a suite that has to read or
/// set a fact of the account that no operation of this service writes.
pub fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Pinned off so a suite does not depend on the process-wide auth switch.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// The response of one call: HTTP status, the `x-amzn-errortype` the protocol carries on
/// a modelled failure, and the parsed JSON body.
pub struct Answer {
    pub status: u16,
    pub error_type: String,
    pub body: Value,
}

impl Answer {
    pub fn at(&self, pointer: &str) -> &Value {
        self.body.pointer(pointer).unwrap_or(&Value::Null)
    }

    pub fn string_at(&self, pointer: &str) -> String {
        self.at(pointer).as_str().unwrap_or_default().to_string()
    }

    /// The message an error body carries, whichever casing the protocol rendered it in.
    pub fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

pub async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    call_with_headers(router, method, uri, body, &[]).await
}

/// One call made the way the console's sign-up flow makes it: carrying the console CSRF
/// header, which is what a console-only operation admits.
pub async fn console_call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    call_with_headers(
        router,
        method,
        uri,
        body,
        &[(CONSOLE_CSRF_HEADER, CONSOLE_CSRF_TOKEN)],
    )
    .await
}

/// One call carrying `headers` beside the ones every call in this crate sends.
pub async fn call_with_headers(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
    headers: &[(&str, &str)],
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "social-messaging.us-east-1.amazonaws.com")
        .header("authorization", SIGNED);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Bytes::from(serde_json::to_vec(&value).expect("the request body serializes"))
        }
        None => Bytes::new(),
    };
    let request = builder.body(payload).expect("the request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            json!({ "__unparsed": String::from_utf8_lossy(&bytes).to_string() })
        })
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// Percent-encode a value for a query string, so an ARN-form `id` (which carries `:` and
/// `/`) survives the round trip. `NON_ALPHANUMERIC` is the conservative set the other
/// harnesses in this tree encode with (see
/// `services/service-com-amazonaws-accessanalyzer-accessanalyzer/tests/all/harness.rs`).
pub fn query_escape(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

/// Start a signup and return the `associateInProgressToken` the callback issued — the
/// only token a `setupFinalization` can finalize.
pub async fn start_signup(router: &MultiServiceRouter) -> String {
    let callback = console_call(
        router,
        "POST",
        "/v1/whatsapp/signup",
        Some(json!({ "signupCallback": { "accessToken": "meta-access-token" } })),
    )
    .await;
    assert_eq!(
        callback.status, 200,
        "signupCallback: expected 200, got {} {:?}",
        callback.status, callback.body
    );
    callback.string_at("/signupCallbackResult/associateInProgressToken")
}

/// A setup-finalization body finalizing `token` and linking a new account with
/// `meta_numbers` registered under it.
pub fn setup_body(token: &str, waba_id: Option<&str>, meta_numbers: &[&str]) -> Value {
    let phone_numbers: Vec<Value> = meta_numbers
        .iter()
        .map(|id| json!({ "id": id, "twoFactorPin": "123456" }))
        .collect();
    let mut setup = json!({
        "associateInProgressToken": token,
        "phoneNumbers": phone_numbers,
    });
    if let Some(waba_id) = waba_id {
        setup["waba"] = json!({ "id": waba_id });
    }
    json!({ "setupFinalization": setup })
}

/// Link one account and return the id the service minted for it: a signup callback hands
/// back the token its finalization has to carry.
pub async fn link(router: &MultiServiceRouter, waba_id: &str, meta_numbers: &[&str]) -> String {
    let token = start_signup(router).await;
    let answer = console_call(
        router,
        "POST",
        "/v1/whatsapp/signup",
        Some(setup_body(&token, Some(waba_id), meta_numbers)),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {waba_id:?}: expected 200 from AssociateWhatsAppBusinessAccount, got {} {:?}",
        answer.status, answer.body
    );
    answer.string_at("/linkedWhatsAppBusinessAccountId")
}
