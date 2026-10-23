// SPDX-License-Identifier: CC0-1.0

//! Shared harness for the Performance Insights suites: a one-service emulator
//! driven over this service's real wire protocol (`awsJson1_1`, `POST /`,
//! `X-Amz-Target: PerformanceInsightsv20180227.<Operation>`), so a test exercises
//! the same deserialize -> validate -> handle -> serialize walk a client does.

#![allow(dead_code)]

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service answers (README.md / OPERATIONS.md).
const TARGET_PREFIX: &str = "PerformanceInsightsv20180227";

/// The region a credential scope this harness builds names — and so the region
/// the ARNs this service mints for a request of this harness sit in. Keep the
/// scope built by `authorization` and this constant in step.
pub const REGION: &str = "us-west-2";

/// A fresh emulator holding only this service, over one shared engine.
pub fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator, with the engine it was built on.
///
/// For a suite that must seed a store ANOTHER service writes — the shared
/// data-source directory the database control planes record a minted DB resource id
/// in — which a one-service emulator has no operation to write.
pub fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_pi_performanceinsightsv20180227::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// A fresh emulator holding this service AND the two services a caller's
/// identity comes from — IAM and STS — over ONE shared engine.
///
/// Authorization is decided from the account's own IAM records, so a test that
/// exercises it has to create the identity through those services' real wire
/// paths, which means every service must be mounted on the same engine, exactly
/// as the router mounts them.
pub fn emulator_with_identity_services() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_pi_performanceinsightsv20180227::EmulatedService::new(engine.clone()),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            engine.clone(),
        ),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// One answer off the wire: the HTTP status and the parsed JSON body.
#[derive(Debug, Clone)]
pub struct Answer {
    pub status: u16,
    pub body: Value,
}

impl Answer {
    /// The wire error code (`__type`), without any namespace or shape prefix.
    /// Empty for a success.
    pub fn error_code(&self) -> String {
        let raw = self
            .body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        raw.rsplit(['#', ':']).next().unwrap_or(raw).to_string()
    }

    /// The error message, for a failure that carries one.
    pub fn error_message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A member of a successful response body.
    pub fn member(&self, name: &str) -> &Value {
        self.body.get(name).unwrap_or(&Value::Null)
    }
}

/// Call one operation and read the answer.
pub async fn call(emulator: &MultiServiceRouter, operation: &str, input: Value) -> Answer {
    call_signed_by(emulator, DEFAULT_ACCESS_KEY_ID, operation, input).await
}

/// The access key id [`call`] presents. It is not one IAM minted for a user, so
/// it is the ambient account identity — the one every suite that is not about
/// authorization drives requests as.
pub const DEFAULT_ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// Call one operation presented as signed by `access_key_id`, and read the
/// answer.
///
/// Request authentication is switched off in the harness, so the signature is
/// never verified; the credential scope is the only place the signing key id
/// appears on the wire, and it is what the service reads the caller's identity
/// from (`emulator_server::extract_akid`).
pub async fn call_signed_by(
    emulator: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    input: Value,
) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "pi.us-west-2.amazonaws.com")
        .header("authorization", authorization(access_key_id, "pi"))
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("content-type", "application/x-amz-json-1.1")
        .body(Bytes::from(input.to_string()))
        .expect("the harness builds a well-formed request");

    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            Value::String(String::from_utf8_lossy(&bytes).to_string())
        })
    };
    Answer { status, body }
}

/// One awsQuery call against the IAM service mounted beside this one: `Action`
/// plus form parameters, answered with the HTTP status and the XML body.
pub async fn iam_call(
    emulator: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    query_call(emulator, "iam", "2010-05-08", action, params).await
}

/// The same, against the STS service mounted beside this one.
pub async fn sts_call(
    emulator: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    query_call(emulator, "sts", "2011-06-15", action, params).await
}

/// One awsQuery call against the service whose signing name is `signing_name`.
async fn query_call(
    emulator: &MultiServiceRouter,
    signing_name: &str,
    version: &str,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.amazonaws.com"))
        .header(
            "authorization",
            authorization(DEFAULT_ACCESS_KEY_ID, signing_name),
        )
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("the harness builds a well-formed request");
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8(response.into_body().to_vec()).expect("a UTF-8 response body");
    (status, body)
}

/// A SigV4 credential scope naming `access_key_id` and `signing_name`, in
/// [`REGION`]. The signing name is the signal the router resolves a service
/// from, because an `X-Amz-Target` prefix names the target
/// (`PerformanceInsightsv20180227`), not the signing name; request
/// authentication is switched off in the harness, so the signature itself is
/// never read.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host;x-amz-date, Signature=00"
    )
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving
/// only RFC 3986's unreserved characters literal.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// The text of the single `<tag>` element an awsQuery response carries.
///
/// A scan rather than a document model, as the sibling awsQuery suites do
/// (`services/service-com-amazonaws-iam-awsidentitymanagementv20100508/tests/all/user_lifecycle.rs`):
/// every tag read here is a LEAF whose content is text.
pub fn xml_element(body: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("<{tag}> is absent from {body}"));
    let rest = &body[start + open.len()..];
    let end = rest
        .find(&close)
        .unwrap_or_else(|| panic!("<{tag}> is never closed in {body}"));
    rest[..end].to_string()
}

/// Whether `id` satisfies the modelled `AnalysisReportId` pattern
/// `^report-[0-9a-f]{17}$`.
pub fn is_modelled_report_id(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("report-") else {
        return false;
    };
    suffix.len() == 17 && suffix.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Whether `token` satisfies the modelled `NextToken` pattern
/// `^[a-zA-Z0-9_=-]+$` and its `length` bound of 1..8192.
pub fn is_modelled_next_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 8192
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'=' || b == b'-')
}
