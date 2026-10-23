// SPDX-License-Identifier: CC0-1.0

//! The one HTTP client the Comprehend suites drive the service through.
//!
//! Every suite goes over the wire the service actually serves — `awsJson1_1`,
//! `POST /` with `X-Amz-Target: Comprehend_20171127.<OperationName>` — through the
//! same `MultiServiceRouter` the emulator's front doors dispatch on, so a test
//! exercises routing, deserialization, constraint validation, the handler and
//! serialization end to end.

use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_comprehend_comprehend_20171127::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (OPERATIONS.md, "Wire protocol").
const TARGET_PREFIX: &str = "Comprehend_20171127.";

/// The region the test's SigV4 credential scope names. Deliberately not the
/// emulator's default, so an ARN that carries it proves the region came from the
/// request rather than from a constant.
pub const REGION: &str = "eu-west-1";

/// The account id every ARN a generated emulator mints carries.
pub const ACCOUNT_ID: &str = "000000000000";

/// One emulator holding one Comprehend service over one shared engine.
pub struct Comprehend {
    router: MultiServiceRouter,
    /// The SAME engine the registered service's handlers reach through
    /// `self.engine`, kept so a suite can seed the store for an operation whose
    /// creating sibling is still a scaffold stub.
    engine: Arc<engine::InMemoryEngine>,
}

impl Comprehend {
    /// A fresh emulator with an empty store.
    pub fn new() -> Self {
        Self::over_engine(Arc::new(engine::InMemoryEngine::new()), false)
    }

    /// A fresh emulator with the S3 service mounted beside Comprehend on the SAME
    /// engine — what the assembled emulator does.
    ///
    /// `StartDominantLanguageDetectionJob` reads its input documents from the Amazon
    /// S3 location `InputDataConfig` names and writes its output archive back to the
    /// one `OutputDataConfig` names, by calling S3 in process, so the suites that
    /// exercise a job's run need object storage answering.
    pub fn with_object_storage() -> Self {
        Self::over_engine(Arc::new(engine::InMemoryEngine::new()), true)
    }

    /// The one construction path both constructors use, so they cannot drift.
    fn over_engine(engine: Arc<engine::InMemoryEngine>, with_object_storage: bool) -> Self {
        let mut router = MultiServiceRouter::new();
        // The suites assert on service behaviour, not on signature verification, and
        // sign with a fixed scope so the region is deterministic.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        if with_object_storage {
            router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
                Arc::clone(&engine),
            ));
        }
        Self { router, engine }
    }

    /// The shared engine behind this emulator, for a suite that has to seed the
    /// store directly. Reads and writes go through
    /// `service_com_amazonaws_comprehend_comprehend_20171127::state::service_state`,
    /// exactly as a handler's do, so a seeded record is the same record a handler
    /// serves.
    pub fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    /// Call `operation` with `body`, returning the HTTP status and the parsed JSON
    /// response (`Value::Null` for an empty body).
    pub async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/comprehend/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
                ),
            )
            .body(::bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or_else(|error| {
                panic!(
                    "{operation}: response body is not JSON ({error}): {:?}",
                    String::from_utf8_lossy(response.body())
                )
            })
        };
        (status, parsed)
    }

    /// Send one path-style request to the emulator's S3 front door, returning the
    /// HTTP status and the raw response body.
    ///
    /// S3 is a REST service: the bucket and key live in the URI and the body is the
    /// object's own bytes, so this is a raw request rather than an `x-amz-target`
    /// call. The credential scope names `s3`, which is what the router claims on.
    pub async fn s3(&self, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
        let request = ::http::Request::builder()
            .method(method)
            .uri(path)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/s3/\
                     aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(::bytes::Bytes::copy_from_slice(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            response.body().as_ref().to_vec(),
        )
    }

    /// Call `operation` expecting success, returning the parsed output.
    pub async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(status, 200, "{operation}: expected 200, got {status}: {parsed}");
        parsed
    }
}

/// The wire error code of a failure response (`__type` on the awsJson protocols),
/// with any `namespace#` prefix stripped.
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The message of a failure response.
///
/// Comprehend's modelled errors spell the member `Message`, as the model does,
/// while the framework's own refusals (`ValidationException`) spell it `message`;
/// both are read here so a suite asserts on the sentence without caring which
/// layer produced it.
pub fn error_message(body: &Value) -> String {
    for key in ["Message", "message"] {
        if let Some(message) = body.get(key).and_then(Value::as_str) {
            return message.to_string();
        }
    }
    "<no message>".to_string()
}

/// The ONE size-report sentence every real-time inference operation refuses an
/// oversized document with (`crate::state::text_size_refusal`), as a suite expects to
/// read it back: built from the request, so it names the quota that applies and the
/// submitted document's UTF-8 byte length.
///
/// Lives here rather than in one suite so no two suites can drift apart on the
/// sentence they assert.
pub fn expected_text_size_message(quota_bytes: usize, actual_bytes: usize) -> String {
    format!(
        "Input text size exceeds limit. Max length of request text allowed is \
         {quota_bytes} bytes while in this request the text size is {actual_bytes} bytes"
    )
}
