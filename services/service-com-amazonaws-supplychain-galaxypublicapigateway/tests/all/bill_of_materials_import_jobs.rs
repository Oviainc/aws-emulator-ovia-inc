// SPDX-License-Identifier: CC0-1.0

//! `CreateBillOfMaterialsImportJob` and `GetBillOfMaterialsImportJob`, driven over the
//! wire (restJson1, signing name `scn`) through the same `MultiServiceRouter` the
//! emulator's front door uses — so every assertion here is about what a real client
//! observes.
//!
//! THE RULE UNDER TEST: the Product BOM import API is WITHDRAWN at the service, so BOTH
//! of its operations refuse EVERY call with the modelled `AccessDeniedException`
//! (HTTP 403) carrying the one fixed sentence AWS Supply Chain states for it — before the
//! request's `instanceId`, its `jobId`, its `s3uri` or its `clientToken` is examined —
//! and nothing comes into existence as a result.
//!
//! The tables span what the rule generalises over: which of the two operations is called,
//! which instance the call is addressed to (one this account holds, one it does not),
//! which `ConfigurationS3Uri` form names the CSV (either case of the scheme the modelled
//! pattern admits, a bare key, a nested prefix, a dotted bucket, a key with no dot at
//! all), which `jobId` is read, and whether the request carries an idempotency token at
//! all — plus a replay of one, which a withdrawn API also refuses.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_supplychain_galaxypublicapigateway::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service (`scn`), which
/// is how the router picks the service that claims the request. Signature verification is
/// switched off for the test router, so the signature itself is never read.
const SCN_CREDENTIAL: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/scn/aws4_request, SignedHeaders=host, Signature=00";

/// The status the model declares for `AccessDeniedException` (`@httpError(403)`).
const ACCESS_DENIED_STATUS: u16 = 403;

/// The error code a client branches on, which is the modelled shape's own name.
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// The sentence AWS Supply Chain refuses the withdrawn Product BOM import API with,
/// written out HERE rather than read from the crate: the wording is the requirement, so
/// the test states it as the service does and a drift in the constant the handlers answer
/// from fails this suite instead of travelling with it.
const WITHDRAWN_API_SENTENCE: &str = "This API has been deprecated. Please contact ASC support \
                                      team at https://aws.amazon.com/contact-us/ for more \
                                      details.";

/// The emulator under test: one engine, one service, driven through the router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Keep the test independent of how this process was launched: no authenticator,
        // so a request routes on its credential scope alone.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Send one request and return its HTTP status and JSON body.
    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let body_bytes = match &body {
            Some(value) => Bytes::from(serde_json::to_vec(value).expect("serialize request body")),
            None => Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://scn.us-east-1.amazonaws.com{path}"))
            .header("host", "scn.us-east-1.amazonaws.com")
            .header("authorization", SCN_CREDENTIAL)
            .header("content-type", "application/json")
            .body(body_bytes)
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "{method} {path}: body is not JSON ({error}): {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, parsed)
    }

    /// A live instance of this account, created through `CreateInstance`.
    async fn given_instance(&self) -> String {
        let (status, body) = self
            .send(
                "POST",
                "/api/instance",
                Some(json!({ "instanceName": "bom-import-host" })),
            )
            .await;
        assert_eq!(
            status, 200,
            "CreateInstance expected 200, got {status} with {body}"
        );
        body.get("instance")
            .and_then(|instance| instance.get("instanceId"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("CreateInstance carried no instanceId: {body}"))
            .to_string()
    }

    /// `CreateBillOfMaterialsImportJob` for `s3uri`, optionally carrying `token`.
    async fn create_job(
        &self,
        instance_id: &str,
        s3uri: &str,
        token: Option<&str>,
    ) -> (u16, Value) {
        let mut request = json!({ "s3uri": s3uri });
        if let Some(token) = token {
            request["clientToken"] = json!(token);
        }
        self.send(
            "POST",
            &format!("/api/configuration/instances/{instance_id}/bill-of-materials-import-jobs"),
            Some(request),
        )
        .await
    }

    /// `GetBillOfMaterialsImportJob` for `job_id` under `instance_id`.
    async fn get_job(&self, instance_id: &str, job_id: &str) -> (u16, Value) {
        self.send(
            "GET",
            &format!(
                "/api/configuration/instances/{instance_id}/bill-of-materials-import-jobs/{job_id}"
            ),
            None,
        )
        .await
    }
}

/// The error code of a refusal envelope (restJson1 renders it in `__type`).
fn error_code(case: &str, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: refusal carries no `__type`: {body}"))
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` of a refusal envelope.
fn error_message(case: &str, body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: refusal carries no message: {body}"))
        .to_string()
}

/// A `ClientToken` built from `seed`: the shape is modelled `len 33..126`, so the seed is
/// padded to 40 characters.
fn client_token(seed: &str) -> String {
    format!("{seed:0<40}")
}

/// RULE: every call is refused `AccessDeniedException` (403) with the service's one fixed
/// sentence, whatever the request carries — and the sentence states nothing of the request.
#[tokio::test]
async fn every_call_is_refused_with_the_services_withdrawn_api_sentence() {
    let emulator = Emulator::new();
    let live = emulator.given_instance().await;
    // A well-formed `UUID` this account holds no instance for, so the refusal cannot be
    // about the instance either.
    let absent = "00000000-0000-0000-0000-000000000000".to_string();

    // Rows differ along the instance addressed, the `ConfigurationS3Uri` form (both
    // scheme cases the modelled pattern `^[sS]3://…` admits, a bare key, a nested prefix,
    // a dotted bucket, a key carrying no dot) and whether a `clientToken` is sent.
    let rows: Vec<(&str, &String, &str, Option<String>)> = vec![
        (
            "a live instance, lowercase scheme, bare key",
            &live,
            "s3://scn-bom-source/product_bom.csv",
            None,
        ),
        (
            "a live instance, uppercase scheme",
            &live,
            "S3://scn-bom-source/product_bom.csv",
            None,
        ),
        (
            "a live instance, nested prefix key",
            &live,
            "s3://scn-bom-source/year=2026/month=01/product_bom.csv",
            Some(client_token("nested")),
        ),
        (
            "a live instance, dotted bucket",
            &live,
            "s3://scn.bom.source/product_bom.csv",
            Some(client_token("dotted")),
        ),
        (
            "a live instance, a key with no dot",
            &live,
            "s3://scn-bom-source/product_bom",
            None,
        ),
        (
            "an instance this account does not hold",
            &absent,
            "s3://scn-bom-source/product_bom.csv",
            None,
        ),
        (
            "an instance this account does not hold, with a token",
            &absent,
            "S3://scn.bom.source/year=2026/product_bom.csv",
            Some(client_token("absent-instance")),
        ),
    ];

    for (case, instance, s3uri, token) in rows {
        let (status, body) = emulator
            .create_job(instance, s3uri, token.as_deref())
            .await;
        assert_eq!(
            status, ACCESS_DENIED_STATUS,
            "case {case:?}: expected {ACCESS_DENIED_STATUS}, got {status} with {body}"
        );
        assert_eq!(
            error_code(case, &body),
            ACCESS_DENIED_CODE,
            "case {case:?}: expected an {ACCESS_DENIED_CODE} envelope, got {body}"
        );
        let message = error_message(case, &body);
        assert_eq!(
            message, WITHDRAWN_API_SENTENCE,
            "case {case:?}: expected {WITHDRAWN_API_SENTENCE:?}, got {message:?}"
        );
        // The sentence is fixed: nothing the request carried is interpolated into it.
        for echoed in [instance.as_str(), s3uri] {
            assert!(
                !message.contains(echoed),
                "case {case:?}: the sentence must state nothing of the request, but it \
                 carries {echoed:?}: {message:?}"
            );
        }
        if let Some(token) = token.as_deref() {
            assert!(
                !message.contains(token),
                "case {case:?}: the sentence must not carry the clientToken: {message:?}"
            );
        }
    }
}

/// RULE: the refusal is the operation's whole behaviour, so a repeat of the SAME request —
/// including a replay of its idempotency token — is refused identically and nothing the
/// first call left behind changes the second.
#[tokio::test]
async fn a_repeated_call_is_refused_identically() {
    let emulator = Emulator::new();
    let instance = emulator.given_instance().await;
    let token = client_token("replayed");

    let mut answers = Vec::new();
    for attempt in 1..=3 {
        let (status, body) = emulator
            .create_job(
                &instance,
                "s3://scn-bom-source/product_bom.csv",
                Some(&token),
            )
            .await;
        assert_eq!(
            status, ACCESS_DENIED_STATUS,
            "attempt {attempt}: expected {ACCESS_DENIED_STATUS}, got {status} with {body}"
        );
        answers.push(body);
    }
    for (attempt, body) in answers.iter().enumerate().skip(1) {
        assert_eq!(
            body, &answers[0],
            "attempt {}: a withdrawn API answers every call alike, got {body}",
            attempt + 1
        );
    }

    // And the read half of the API answers the same refusal, so no job is reported
    // either: a withdrawn API has none to report.
    let (status, body) = emulator
        .get_job(&instance, "11111111-2222-3333-4444-555555555555")
        .await;
    assert_eq!(
        status, ACCESS_DENIED_STATUS,
        "the read half must be refused too, got {status} with {body}"
    );
    assert_eq!(
        error_message("the read after the repeats", &body),
        WITHDRAWN_API_SENTENCE,
        "the read half states the same sentence, got {body}"
    );
}

/// RULE: the read half of the withdrawn API is refused exactly as the create half is,
/// whatever instance and whatever `jobId` the request names.
#[tokio::test]
async fn every_read_is_refused_with_the_same_sentence() {
    let emulator = Emulator::new();
    let live = emulator.given_instance().await;
    let absent = "00000000-0000-0000-0000-000000000000".to_string();

    // Rows differ along the instance addressed and the `jobId` read: ids of the modelled
    // `UUID` shape that name nothing, since nothing can be named.
    let rows: Vec<(&str, &String, &str)> = vec![
        (
            "a live instance, an all-zero job id",
            &live,
            "00000000-0000-0000-0000-000000000000",
        ),
        (
            "a live instance, an all-f job id",
            &live,
            "ffffffff-ffff-ffff-ffff-ffffffffffff",
        ),
        (
            "a live instance, a mixed-hex job id",
            &live,
            "0123456f-89ab-cdef-0123-456789abcdef",
        ),
        (
            "an instance this account does not hold",
            &absent,
            "11111111-2222-3333-4444-555555555555",
        ),
    ];

    for (case, instance, job_id) in rows {
        let (status, body) = emulator.get_job(instance, job_id).await;
        assert_eq!(
            status, ACCESS_DENIED_STATUS,
            "case {case:?}: expected {ACCESS_DENIED_STATUS}, got {status} with {body}"
        );
        assert_eq!(
            error_code(case, &body),
            ACCESS_DENIED_CODE,
            "case {case:?}: expected an {ACCESS_DENIED_CODE} envelope, got {body}"
        );
        let message = error_message(case, &body);
        assert_eq!(
            message, WITHDRAWN_API_SENTENCE,
            "case {case:?}: expected {WITHDRAWN_API_SENTENCE:?}, got {message:?}"
        );
        for echoed in [instance.as_str(), job_id] {
            assert!(
                !message.contains(echoed),
                "case {case:?}: the sentence must state nothing of the request, but it \
                 carries {echoed:?}: {message:?}"
            );
        }
    }
}
