// SPDX-License-Identifier: CC0-1.0

//! What a read of a REGISTERED stream answers when the request itself is
//! invalid: the modelled-constraint refusal runs ahead of stream resolution, so
//! the clause a caller gets is the same whether or not the stream it names
//! exists.
//!
//! The framework validates the request before any handler runs, and this
//! service words that refusal (`Service::override_refusal`): a constraint
//! broken by the ELEMENTS of a collection member is stated at the member, with
//! the element shape's whole modelled constraint set. Resolution — the registry
//! of the Kinesis Video Streams control plane, then this service's own archive
//! — happens inside the handler, which such a request never reaches. These
//! cases pin that order over all three resolution outcomes: a stream the
//! control plane created, one only this service's archive holds, and one
//! nothing holds.
//!
//! The per-operation behaviour over a registered stream's archive is
//! `control_plane_stream_resolution`'s; the refusal wording itself is
//! `refusal_message_wording`'s. This suite is only about which of the two
//! answers first.

use std::sync::Arc;
use std::time::SystemTime;

use service_com_amazonaws_kinesisvideo_kinesisvideo_20170930 as control_plane;
use service_com_amazonaws_kinesisvideoarchivedmedia_awsacuityreader as archived_media;
use smithy_server_runtime::MultiServiceRouter;

use archived_media::state::{service_state, ArchivedStream};

/// The emulator endpoint every case addresses: this emulator, never a real AWS
/// host.
const ENDPOINT_HOST: &str = "kinesisvideo.us-east-1.amazonaws.com";

/// The region every request in this suite is signed for.
const REGION: &str = "us-east-1";

/// The element constraint set `GetMediaForFragmentList$Fragments` states.
///
/// The model's, not the request's: `Fragments` targets `FragmentNumberList`,
/// whose member targets `FragmentNumberString`
/// (`smithy_query(query="shape",shape="FragmentNumberString")` — `@length(min:
/// 1, max: 128)` and `@pattern("^[0-9]+$")`).
const FRAGMENT_ELEMENT_SET: &str = "Member must satisfy constraint: [Member must have length less \
                                    than or equal to 128, Member must have length greater than or \
                                    equal to 1, Member must satisfy regular expression pattern: \
                                    ^[0-9]+$]";

/// One router over one engine, carrying the archived-media reader AND the
/// Kinesis Video Streams control plane, so a stream created through
/// `CreateStream` is the record a read resolves.
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(control_plane::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(archived_media::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// POST one restJson1 call and answer its status, its error code and its
    /// parsed body.
    async fn call(&self, path: &str, body: serde_json::Value) -> (u16, String, serde_json::Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{ENDPOINT_HOST}{path}"))
            .header("host", ENDPOINT_HOST)
            .header("content-type", "application/json")
            // The SigV4 credential scope routes the request; the signature
            // itself is not checked here.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/kinesisvideo/\
                     aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed: serde_json::Value =
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        let code = parsed
            .get("__type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string();
        (status, code, parsed)
    }

    /// Create `name` through the control plane, so the archived-media reader
    /// resolves it in that registry.
    async fn create_stream(&self, name: &str) {
        let (status, _, body) = self
            .call(
                "/createStream",
                serde_json::json!({ "StreamName": name, "DataRetentionInHours": 24 }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case create {name:?}: expected HTTP 200, got {status} with {body:?}"
        );
    }

    /// Put an empty archive for `name` into the reader's own store, which is
    /// how a stream the control plane does not hold is still read.
    fn seed_archive(&self, name: &str) {
        let state = service_state(&*self.engine).expect("the archived-media store");
        state
            .lock()
            .put_stream(ArchivedStream::new(name, REGION, SystemTime::now(), 24));
    }
}

/// The `message` a refusal carries, whichever member the protocol put it in.
fn message(body: &serde_json::Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// RULE: a modelled constraint the request breaks is refused before the stream
/// is resolved, so the clause is the same for a stream the control plane holds,
/// one only this service's archive holds, and one nothing holds — and it is the
/// element-set clause, never a resolution refusal.
///
/// The rows differ along the two things the rule generalises over: where (if
/// anywhere) the named stream exists, and which of the element shape's
/// constraints the request broke.
#[tokio::test]
async fn a_constraint_refusal_answers_before_the_stream_is_resolved() {
    let harness = Harness::new();
    harness.create_stream("registered-stream").await;
    harness.seed_archive("archive-only-stream");

    // (case, the stream named, the fragment element that breaks the set)
    let rows: [(&str, &str, &str); 6] = [
        (
            "a registered stream, a non-numeric element",
            "registered-stream",
            "not-a-fragment-number",
        ),
        (
            "a registered stream, an empty element",
            "registered-stream",
            "",
        ),
        (
            "a stream only the archive holds, a non-numeric element",
            "archive-only-stream",
            "0x2a",
        ),
        (
            "a stream nothing holds, a non-numeric element",
            "no-such-stream",
            "nope",
        ),
        (
            "a stream nothing holds, an empty element",
            "no-such-stream",
            "",
        ),
        (
            // 129 digits: inside the pattern, one past the length maximum.
            "a registered stream, an element past the length maximum",
            "registered-stream",
            "999999999999999999999999999999999999999999999999999999999999999999999999999999999\
             999999999999999999999999999999999999999999999999",
        ),
    ];

    let expected = format!(
        "1 validation error detected: Value at 'fragments' failed to satisfy constraint: \
         {FRAGMENT_ELEMENT_SET}"
    );
    for (case, stream, element) in rows {
        let (status, code, body) = harness
            .call(
                "/getMediaForFragmentList",
                serde_json::json!({ "StreamName": stream, "Fragments": [element] }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            code, "ValidationException",
            "case {case}: expected ValidationException, got {code:?} with {body:?}"
        );
        let stated = message(&body);
        assert_eq!(
            stated, expected,
            "case {case}: expected {expected:?}, got {stated:?}"
        );
    }
}

/// RULE: the same order holds for a request that names no stream at all — the
/// constraint refusal answers, not the stream-addressing one, because the
/// framework's walk runs first.
#[tokio::test]
async fn a_constraint_refusal_answers_before_the_addressing_refusal() {
    let harness = Harness::new();
    let (status, code, body) = harness
        .call(
            "/getMediaForFragmentList",
            serde_json::json!({ "Fragments": ["not-a-fragment-number"] }),
        )
        .await;
    assert_eq!(
        status, 400,
        "expected HTTP 400, got {status} with {body:?}"
    );
    assert_eq!(
        code, "ValidationException",
        "expected ValidationException, got {code:?} with {body:?}"
    );
    let stated = message(&body);
    let expected = format!(
        "1 validation error detected: Value at 'fragments' failed to satisfy constraint: \
         {FRAGMENT_ELEMENT_SET}"
    );
    assert_eq!(
        stated, expected,
        "expected {expected:?}, got {stated:?}"
    );
}
