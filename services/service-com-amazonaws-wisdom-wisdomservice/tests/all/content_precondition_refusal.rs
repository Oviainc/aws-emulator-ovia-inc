// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateContent`'s optimistic lock across the whole range of
//! `revisionId` values the model admits, driven over HTTP through the same router the
//! production emulator mounts.
//!
//! The rule under test: `UpdateContent` is served from stored state for EVERY request the
//! model admits — no request reaches a not-implemented answer — and `revisionId` is the
//! optimistic lock the member documents, so a request naming a revision the content no
//! longer carries is refused HTTP 412 `PreconditionFailedException` and writes nothing,
//! whatever shape that value takes, while a request naming the revision the record does
//! carry is applied and moves the content to a new one. A value outside the member's
//! modelled length is not a precondition at all: it is refused where the constraint is
//! checked, and also writes nothing.
//!
//! Asserted over a TABLE of `revisionId` values spanning the class — a superseded
//! revision, an id form this service mints that names nothing, and the member's own
//! published length bounds plus one value past the upper one — rather than one recorded
//! request.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_wisdom_wisdomservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for.
const REGION: &str = "us-west-2";

/// The endpoint host those requests address.
const ENDPOINT_HOST: &str = "wisdom.us-west-2.amazonaws.com";

/// The routing key this service answers under (`src/service.rs`'s
/// `OWN_REGISTRATION_KEY`).
const ROUTE_KEY: &str = "com-amazonaws-wisdom-wisdomservice";

/// A UUID no create ever mints, for the revision that names nothing.
const ABSENT_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// The only `KnowledgeBaseType` a content may be uploaded into.
const TYPE_CUSTOM: &str = "CUSTOM";

/// A media type from the modelled `ContentType` catalog
/// (`smithy_query(query="shape",shape="ContentType")`).
const TYPE_TEXT: &str = "text/plain";

/// The modelled length of `revisionId` (`NonEmptyString`, from
/// `smithy_query(query="shape",shape="NonEmptyString")`: length 1..4096), so the table can
/// name each published bound and one value past the upper one.
const REVISION_LEN_MIN: usize = 1;
const REVISION_LEN_MAX: usize = 4096;

/// The two refusal classes this suite distinguishes.
const PRECONDITION_FAILED: &str = "PreconditionFailedException";
const VALIDATION: &str = "ValidationException";

/// The code a scaffolded operation answers with, which an implemented one never reaches:
/// no request in this suite may be answered by it.
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// One wire answer, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: String,
    body: Value,
}

impl Reply {
    fn string(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The object at `pointer`, or an empty one when the answer carries none.
    fn object(&self, pointer: &str) -> Map<String, Value> {
        self.body
            .pointer(pointer)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    }

    /// The error code a client branches on: the `x-amzn-errortype` header, falling back to
    /// the body's `__type` for a protocol that carries it only there.
    fn code(&self) -> String {
        if self.error_type.is_empty() {
            self.string("/__type")
        } else {
            self.error_type.clone()
        }
    }

}

/// Dispatch one already-built request and read the answer.
async fn dispatch(router: &MultiServiceRouter, request: http::Request<Bytes>) -> Reply {
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
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_type,
        body,
    }
}

/// An emulator with just this service mounted on a fresh engine, driven in-process
/// through the production router.
struct Emulator {
    router: Arc<MultiServiceRouter>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes by the
        // SigV4 credential scope below.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self {
            router: Arc::new(router),
        }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{ROUTE_KEY}/aws4_request, \
             SignedHeaders=host, Signature=not-verified-in-this-suite"
        );
        let builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{ENDPOINT_HOST}{uri}"))
            .header("host", ENDPOINT_HOST)
            .header("authorization", authorization);
        let request = match &body {
            Some(value) => builder
                .header("content-type", "application/json")
                .body(Bytes::from(
                    serde_json::to_vec(value).expect("serialize body"),
                ))
                .expect("build request"),
            None => builder.body(Bytes::new()).expect("build request"),
        };
        dispatch(&self.router, request).await
    }

    /// Follow a URL this service handed out exactly as a client does, with NO credential.
    async fn follow(&self, url: &str, content_type: &str, body: &[u8]) -> Reply {
        let request = http::Request::builder()
            .method("PUT")
            .uri(url)
            .header("content-type", content_type)
            .body(Bytes::copy_from_slice(body))
            .expect("build request");
        dispatch(&self.router, request).await
    }

    /// A `CUSTOM` knowledge base, as its create persisted it.
    async fn knowledge_base(&self, name: &str) -> String {
        let reply = self
            .send(
                "POST",
                "/knowledgeBases",
                Some(json!({ "name": name, "knowledgeBaseType": TYPE_CUSTOM })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"knowledge base {name}\": expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        reply.string("/knowledgeBase/knowledgeBaseId")
    }

    /// A content holding `asset`, as its create persisted it: `(contentId, revisionId)`.
    async fn content(&self, knowledge_base_id: &str, name: &str, asset: &[u8]) -> (String, String) {
        let started = self
            .send(
                "POST",
                &format!("/knowledgeBases/{knowledge_base_id}/upload"),
                Some(json!({ "contentType": TYPE_TEXT })),
            )
            .await;
        assert_eq!(
            started.status, 200,
            "case \"upload for {name}\": expected HTTP 200, got {} with {}",
            started.status, started.body
        );
        let put = self
            .follow(&started.string("/url"), TYPE_TEXT, asset)
            .await;
        assert_eq!(
            put.status, 200,
            "case \"PUT the asset of {name}\": expected HTTP 200, got {} with {}",
            put.status, put.body
        );
        let created = self
            .send(
                "POST",
                &format!("/knowledgeBases/{knowledge_base_id}/contents"),
                Some(json!({
                    "name": name,
                    "uploadId": started.string("/uploadId"),
                    "title": format!("the title of {name}"),
                })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case \"content {name}\": expected HTTP 200, got {} with {}",
            created.status, created.body
        );
        (
            created.string("/content/contentId"),
            created.string("/content/revisionId"),
        )
    }

    async fn get_content(&self, knowledge_base_id: &str, content_id: &str) -> Map<String, Value> {
        self.send(
            "GET",
            &format!("/knowledgeBases/{knowledge_base_id}/contents/{content_id}"),
            None,
        )
        .await
        .object("/content")
    }

    async fn update_content(
        &self,
        knowledge_base_id: &str,
        content_id: &str,
        body: Value,
    ) -> Reply {
        self.send(
            "POST",
            &format!("/knowledgeBases/{knowledge_base_id}/contents/{content_id}"),
            Some(body),
        )
        .await
    }
}

/// One row of the refusal table: the `revisionId` the request names, and the answer this
/// endpoint owes it.
struct Row {
    case: &'static str,
    revision: String,
    status: u16,
    code: &'static str,
}

/// RULE: every `UpdateContent` the model admits is served from stored state — none is
/// answered as unimplemented — and a request naming a revision the record does not carry
/// is refused the modelled 412, whatever shape that value takes, leaving the record
/// exactly as it was; a value outside the member's modelled length is refused at the
/// constraint boundary and leaves it alone too.
#[tokio::test]
async fn a_failed_precondition_is_refused_across_every_revision_form_and_writes_nothing() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.knowledge_base("precondition-answer-kb").await;
    let (content_id, first_revision) = emulator
        .content(&knowledge_base_id, "handbook", b"the first asset")
        .await;

    // One write lands, which supersedes the revision the create reported.
    let landed = emulator
        .update_content(
            &knowledge_base_id,
            &content_id,
            json!({ "title": "The landed title", "revisionId": first_revision }),
        )
        .await;
    assert_eq!(
        landed.status, 200,
        "case \"the first write\": expected HTTP 200, got {} with {}",
        landed.status, landed.body
    );
    let current_revision = landed.string("/content/revisionId");
    let before = emulator.get_content(&knowledge_base_id, &content_id).await;

    let rows = vec![
        Row {
            case: "the revision the content carried before the write",
            revision: first_revision.clone(),
            status: 412,
            code: PRECONDITION_FAILED,
        },
        Row {
            case: "a revision of this service's id form naming nothing",
            revision: ABSENT_UUID.to_string(),
            status: 412,
            code: PRECONDITION_FAILED,
        },
        Row {
            case: "the shortest value the member's length admits",
            revision: "x".repeat(REVISION_LEN_MIN),
            status: 412,
            code: PRECONDITION_FAILED,
        },
        Row {
            case: "the longest value the member's length admits",
            revision: "x".repeat(REVISION_LEN_MAX),
            status: 412,
            code: PRECONDITION_FAILED,
        },
        Row {
            // One past the member's upper bound is not a precondition at all: it is
            // refused where the constraint is checked, before any revision is compared.
            case: "one character past the member's longest admitted value",
            revision: "x".repeat(REVISION_LEN_MAX + 1),
            status: 400,
            code: VALIDATION,
        },
    ];

    for Row {
        case,
        revision,
        status,
        code,
    } in rows
    {
        let refused = emulator
            .update_content(
                &knowledge_base_id,
                &content_id,
                json!({ "title": "A title the loser wanted", "revisionId": revision }),
            )
            .await;
        assert_eq!(
            (refused.status, refused.code().as_str()),
            (status, code),
            "case {case:?}: expected HTTP {status} {code}, got {} {:?} with {}",
            refused.status,
            refused.code(),
            refused.body
        );
        assert_ne!(
            refused.code(),
            NOT_IMPLEMENTED,
            "case {case:?}: this operation is served from state, so no request may be \
             answered as unimplemented, got {}",
            refused.body
        );
        let after = emulator.get_content(&knowledge_base_id, &content_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused write must leave the record as it was, got {after:?}"
        );
    }

    // The control: the revision the record actually carries is applied, and the write is
    // the one a later read reports.
    let accepted = emulator
        .update_content(
            &knowledge_base_id,
            &content_id,
            json!({ "title": "Named the current revision", "revisionId": current_revision }),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "case \"naming the current revision\": expected HTTP 200, got {} with {}",
        accepted.status, accepted.body
    );
    let read_back = emulator.get_content(&knowledge_base_id, &content_id).await;
    assert_eq!(
        read_back.get("title").and_then(Value::as_str),
        Some("Named the current revision"),
        "case \"naming the current revision\": the write must be readable back, got \
         {read_back:?}"
    );
    assert_eq!(
        read_back,
        accepted.object("/content"),
        "case \"naming the current revision\": the answer must be the record a later read \
         reports, got {read_back:?}"
    );
    assert_ne!(
        read_back.get("revisionId").and_then(Value::as_str),
        Some(current_revision.as_str()),
        "case \"naming the current revision\": a write that lands must mint a new \
         revision, got {read_back:?}"
    );
}
