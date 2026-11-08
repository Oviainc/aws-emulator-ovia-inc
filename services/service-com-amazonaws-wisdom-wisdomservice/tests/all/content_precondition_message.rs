// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the PAYLOAD of `UpdateContent`'s optimistic-lock refusal, driven
//! over HTTP through the same router the production emulator mounts.
//!
//! The rule under test: this endpoint's stale-revision refusal is its HTTP status and its
//! error code and NOTHING else — the modelled error's `message` member is optional and
//! this endpoint sends none, so the member is absent from the body and a client reads an
//! empty message, whatever shape the superseded `revisionId` took. The refusal classes
//! that DO carry a sentence (the modelled not-found and input failures of the same
//! operation) still carry theirs, so the absence is a property of this one error class
//! rather than of the operation.
//!
//! Asserted over a TABLE of `revisionId` values spanning the class — a revision superseded
//! once, one superseded twice, another content's current revision, an id form this service
//! mints that names nothing, and each published bound of the member's own length — rather
//! than one recorded request.

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

/// A UUID no create ever mints, for the revision and the labels that name nothing.
const ABSENT_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// The only `KnowledgeBaseType` a content may be uploaded into.
const TYPE_CUSTOM: &str = "CUSTOM";

/// A media type from the modelled `ContentType` catalog
/// (`smithy_query(query="shape",shape="ContentType")`).
const TYPE_TEXT: &str = "text/plain";

/// The modelled length of `revisionId` (`NonEmptyString`, from
/// `smithy_query(query="shape",shape="NonEmptyString")`: length 1..4096), so the table can
/// name each published bound of the member.
const REVISION_LEN_MIN: usize = 1;
const REVISION_LEN_MAX: usize = 4096;

/// The refusal classes this suite distinguishes.
const PRECONDITION_FAILED: &str = "PreconditionFailedException";
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";
const VALIDATION: &str = "ValidationException";

/// The member the modelled error declares (optional, target `String`) and that this
/// refusal class leaves unset.
const MEMBER_MESSAGE: &str = "message";

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

    /// Whether the body carries the error's `message` member at all — what decides the
    /// empty `Error.Message` a client reads.
    fn carries_message(&self) -> bool {
        self.body
            .as_object()
            .is_some_and(|body| body.contains_key(MEMBER_MESSAGE))
    }

    /// The text of that member, which is empty when the member is absent.
    fn message(&self) -> String {
        self.string("/message")
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

    /// An upload of this knowledge base holding `asset`, as a receipt a later call spends.
    async fn upload(&self, knowledge_base_id: &str, asset: &[u8]) -> String {
        let started = self
            .send(
                "POST",
                &format!("/knowledgeBases/{knowledge_base_id}/upload"),
                Some(json!({ "contentType": TYPE_TEXT })),
            )
            .await;
        assert_eq!(
            started.status, 200,
            "case \"start an upload\": expected HTTP 200, got {} with {}",
            started.status, started.body
        );
        let put = self.follow(&started.string("/url"), TYPE_TEXT, asset).await;
        assert_eq!(
            put.status, 200,
            "case \"PUT the asset\": expected HTTP 200, got {} with {}",
            put.status, put.body
        );
        started.string("/uploadId")
    }

    /// A content holding `asset`, as its create persisted it: `(contentId, revisionId)`.
    async fn content(&self, knowledge_base_id: &str, name: &str, asset: &[u8]) -> (String, String) {
        let upload_id = self.upload(knowledge_base_id, asset).await;
        let created = self
            .send(
                "POST",
                &format!("/knowledgeBases/{knowledge_base_id}/contents"),
                Some(json!({
                    "name": name,
                    "uploadId": upload_id,
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

    async fn update_content(&self, knowledge_base_id: &str, content_id: &str, body: Value) -> Reply {
        self.send(
            "POST",
            &format!("/knowledgeBases/{knowledge_base_id}/contents/{content_id}"),
            Some(body),
        )
        .await
    }

    /// Apply a write that names the revision `content_id` currently carries, and report the
    /// revision it moves to.
    async fn supersede(&self, knowledge_base_id: &str, content_id: &str, revision: &str) -> String {
        let landed = self
            .update_content(
                knowledge_base_id,
                content_id,
                json!({ "title": "a landed title", "revisionId": revision }),
            )
            .await;
        assert_eq!(
            landed.status, 200,
            "case \"a write naming the current revision\": expected HTTP 200, got {} with {}",
            landed.status, landed.body
        );
        landed.string("/content/revisionId")
    }
}

/// One row of the refusal table: the `revisionId` the request names.
struct Row {
    case: &'static str,
    revision: String,
}

/// RULE: the stale-revision refusal of this endpoint is its 412 and its error code alone —
/// the modelled error's optional `message` member is absent from the body, so a client
/// reads an empty message — for every form a superseded `revisionId` can take, and the
/// record is left as it was.
#[tokio::test]
async fn the_stale_revision_refusal_carries_no_message_member() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.knowledge_base("precondition-message-kb").await;
    let (content_id, created_revision) = emulator
        .content(&knowledge_base_id, "handbook", b"the first asset")
        .await;
    // A second content of the same knowledge base, so one row can name a revision that is
    // current somewhere but not here.
    let (_, other_revision) = emulator
        .content(&knowledge_base_id, "appendix", b"another asset")
        .await;

    // Two writes land, so the table can name a revision superseded once and one superseded
    // twice over.
    let second_revision = emulator
        .supersede(&knowledge_base_id, &content_id, &created_revision)
        .await;
    let current_revision = emulator
        .supersede(&knowledge_base_id, &content_id, &second_revision)
        .await;
    let before = emulator.get_content(&knowledge_base_id, &content_id).await;

    let rows = vec![
        Row {
            case: "the revision the create reported, superseded twice over",
            revision: created_revision.clone(),
        },
        Row {
            case: "the revision the first write minted, superseded once",
            revision: second_revision.clone(),
        },
        Row {
            case: "the current revision of another content of this knowledge base",
            revision: other_revision.clone(),
        },
        Row {
            case: "a revision of this service's id form naming nothing",
            revision: ABSENT_UUID.to_string(),
        },
        Row {
            case: "the shortest value the member's length admits",
            revision: "x".repeat(REVISION_LEN_MIN),
        },
        Row {
            case: "the longest value the member's length admits",
            revision: "x".repeat(REVISION_LEN_MAX),
        },
    ];

    for Row { case, revision } in rows {
        let refused = emulator
            .update_content(
                &knowledge_base_id,
                &content_id,
                json!({ "title": "a title the loser wanted", "revisionId": revision }),
            )
            .await;
        assert_eq!(
            (refused.status, refused.code().as_str()),
            (412, PRECONDITION_FAILED),
            "case {case:?}: expected HTTP 412 {PRECONDITION_FAILED}, got {} {:?} with {}",
            refused.status,
            refused.code(),
            refused.body
        );
        assert!(
            !refused.carries_message(),
            "case {case:?}: expected no {MEMBER_MESSAGE} member in the refusal, got {:?} in {}",
            refused.message(),
            refused.body
        );
        assert_eq!(
            refused.message(),
            "",
            "case {case:?}: expected an empty message, got {:?} in {}",
            refused.message(),
            refused.body
        );
        let after = emulator.get_content(&knowledge_base_id, &content_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused write must leave the record as it was, got {after:?}"
        );
    }

    // The control: the revision the record actually carries is applied, and that answer
    // carries no error payload at all.
    let accepted = emulator
        .update_content(
            &knowledge_base_id,
            &content_id,
            json!({ "title": "named the current revision", "revisionId": current_revision }),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "case \"naming the current revision\": expected HTTP 200, got {} with {}",
        accepted.status, accepted.body
    );
    assert_eq!(
        emulator
            .get_content(&knowledge_base_id, &content_id)
            .await
            .get("title")
            .and_then(Value::as_str),
        Some("named the current revision"),
        "case \"naming the current revision\": the write must be readable back"
    );
}

/// One row of the other-classes table: a request this operation refuses for a reason OTHER
/// than the optimistic lock, and the class it owes.
struct OtherRow {
    case: &'static str,
    knowledge_base_label: String,
    content_label: String,
    body: Value,
    status: u16,
    code: &'static str,
}

/// RULE: the empty body belongs to the optimistic-lock class alone — the other modelled
/// refusals of the SAME operation keep the sentence they answer with, so a client can still
/// branch on them.
#[tokio::test]
async fn the_other_refusals_of_this_operation_still_carry_their_message() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.knowledge_base("precondition-scope-kb").await;
    let (content_id, _) = emulator
        .content(&knowledge_base_id, "scoped", b"the first asset")
        .await;

    let rows = vec![
        OtherRow {
            case: "a knowledge base this account does not hold",
            knowledge_base_label: ABSENT_UUID.to_string(),
            content_label: content_id.clone(),
            body: json!({ "title": "a title" }),
            status: 404,
            code: RESOURCE_NOT_FOUND,
        },
        OtherRow {
            case: "a content this knowledge base does not hold",
            knowledge_base_label: knowledge_base_id.clone(),
            content_label: ABSENT_UUID.to_string(),
            body: json!({ "title": "a title" }),
            status: 404,
            code: RESOURCE_NOT_FOUND,
        },
        OtherRow {
            case: "both halves of one paired member",
            knowledge_base_label: knowledge_base_id.clone(),
            content_label: content_id.clone(),
            body: json!({
                "overrideLinkOutUri": "https://example.test/articles/elsewhere",
                "removeOverrideLinkOutUri": true,
            }),
            status: 400,
            code: VALIDATION,
        },
        OtherRow {
            case: "an uploadId this service did not issue",
            knowledge_base_label: knowledge_base_id.clone(),
            content_label: content_id.clone(),
            body: json!({ "uploadId": ABSENT_UUID }),
            status: 400,
            code: VALIDATION,
        },
    ];

    for OtherRow {
        case,
        knowledge_base_label,
        content_label,
        body,
        status,
        code,
    } in rows
    {
        let refused = emulator
            .update_content(&knowledge_base_label, &content_label, body)
            .await;
        assert_eq!(
            (refused.status, refused.code().as_str()),
            (status, code),
            "case {case:?}: expected HTTP {status} {code}, got {} {:?} with {}",
            refused.status,
            refused.code(),
            refused.body
        );
        assert!(
            refused.carries_message() && !refused.message().is_empty(),
            "case {case:?}: this class answers with a sentence, got {:?} in {}",
            refused.message(),
            refused.body
        );
    }
}
