// SPDX-License-Identifier: CC0-1.0

//! The emulator every suite in this crate drives, and the encoding its
//! structured headers carry.
//!
//! Each test builds the service over one shared engine, registers it with the
//! real [`MultiServiceRouter`], and dispatches buffered HTTP requests — so what
//! a suite asserts is wire behaviour (status, headers, body) and not a handler
//! called directly.

use bytes::Bytes;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;
use std::io::{Read, Write};
use std::sync::Arc;

use service_com_amazonaws_lexruntimev2_awsdeepsenseruntimeserviceapi2_0::EmulatedService;

/// The emulator under test: one shared engine, one registered service, driven
/// through the router every front door uses.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are the framework's business, not this suite's.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    pub(crate) async fn send(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        self.send_with_headers(method, path, body, &[]).await
    }

    /// [`Self::send`] with extra request headers, so a member the model binds
    /// with `@httpHeader` is exercised through the binding an SDK client uses
    /// rather than through the body.
    pub(crate) async fn send_with_headers(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> Reply {
        let payload = body.map(|b| b.to_string()).unwrap_or_default();
        self.send_raw(
            method,
            path,
            Bytes::from(payload),
            "application/json",
            headers,
        )
        .await
    }

    /// Dispatch a request whose body is NOT JSON: the `@httpPayload` blob an
    /// operation like `RecognizeUtterance` binds, under the content type the
    /// request declares for it.
    pub(crate) async fn send_raw(
        &self,
        method: &str,
        path: &str,
        body: Bytes,
        content_type: &str,
        headers: &[(&str, &str)],
    ) -> Reply {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://runtime-v2-lex.us-east-1.amazonaws.com{path}"
            ))
            // The routing key: the SigV4 credential scope names the `lex`
            // signing name, which is how a request reaches this service.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260102/us-east-1/lex/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef",
            )
            .header("content-type", content_type);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(body)
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = response.into_body();
        Reply {
            status,
            headers,
            body,
        }
    }
}

pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) headers: http::HeaderMap,
    pub(crate) body: Bytes,
}

impl Reply {
    pub(crate) fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "response body is not JSON ({e}): {:?}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    pub(crate) fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .map(|v| v.to_str().expect("header is ASCII").to_string())
    }

    /// The error shape a failed call names, from the restJson1 `__type`.
    pub(crate) fn error_type(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<no __type>")
            .to_string()
    }

    /// Decode one `x-amz-lex-*` header carrying a structured payload: the
    /// documented "compressed with gzip and then base64 encoded" form of the
    /// value's JSON.
    pub(crate) fn lex_header(&self, name: &str) -> Option<Value> {
        let payload = self.lex_header_text(name)?;
        Some(
            serde_json::from_str(&payload)
                .unwrap_or_else(|e| panic!("header {name} is not JSON ({e}): {payload}")),
        )
    }

    /// Decode one `x-amz-lex-*` header to the text it carries, under the same
    /// encoding ([`un_gzip_base64`]).
    pub(crate) fn lex_header_text(&self, name: &str) -> Option<String> {
        let encoded = self.header(name)?;
        Some(un_gzip_base64(name, &encoded))
    }
}

/// gzip then base64, the encoding this service's structured headers carry on
/// the REQUEST side too ("the `sessionState` field must be compressed using gzip
/// and then base64 encoded before sending to Amazon Lex V2").
pub(crate) fn gzip_base64(payload: &str) -> String {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(payload.as_bytes())
        .expect("writing to a vector cannot fail");
    let compressed = encoder.finish().expect("finishing a vector cannot fail");
    aws_smithy_types::base64::encode(compressed)
}

/// base64 then gunzip: the inverse of [`gzip_base64`], panicking with the
/// header's name when the value is not that encoding.
pub(crate) fn un_gzip_base64(name: &str, encoded: &str) -> String {
    let compressed = aws_smithy_types::base64::decode(encoded)
        .unwrap_or_else(|e| panic!("header {name} is not base64 ({e}): {encoded}"));
    let mut payload = Vec::new();
    GzDecoder::new(compressed.as_slice())
        .read_to_end(&mut payload)
        .unwrap_or_else(|e| panic!("header {name} is not gzipped ({e}): {encoded}"));
    String::from_utf8(payload)
        .unwrap_or_else(|e| panic!("header {name} does not decode to UTF-8 ({e}): {encoded}"))
}
