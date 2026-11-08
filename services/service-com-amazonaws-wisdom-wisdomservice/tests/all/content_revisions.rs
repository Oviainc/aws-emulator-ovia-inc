// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateContent`, driven over HTTP through the same router the
//! production emulator mounts.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! * an update WRITES the stored content: every member it names replaces the stored one,
//!   the `remove…` flag clears the member it pairs with, a member it carries neither half
//!   of keeps its value, and the answer is the record a later `GetContent` reports;
//! * `revisionId` is an optimistic lock — a request naming the revision the content
//!   carries is applied and moves it to a NEW revision, one naming a revision it no longer
//!   carries is refused `PreconditionFailedException` (HTTP 412) and writes nothing, and a
//!   request naming none asks for no precondition;
//! * so of several writers naming ONE revision exactly one wins and every other is
//!   refused 412 — asserted at several widths, with the callers genuinely in flight
//!   together;
//! * a knowledge base or a content the route names and this account does not hold is the
//!   modelled `ResourceNotFoundException` (HTTP 404), and a request that both sets and
//!   removes one member, or names an `uploadId` this service did not issue, is
//!   `ValidationException` (HTTP 400).
//!
//! Everything goes over the wire — the asset through the URL the service hands out — so a
//! response that is not computed from persisted state cannot pass.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_wisdom_wisdomservice::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for.
const REGION: &str = "us-west-2";

/// The endpoint host those requests address.
const ENDPOINT_HOST: &str = "wisdom.us-west-2.amazonaws.com";

/// The routing key this service answers under (`src/service.rs`'s
/// `OWN_REGISTRATION_KEY`).
const ROUTE_KEY: &str = "com-amazonaws-wisdom-wisdomservice";

/// A UUID no create ever mints, for the not-found assertions.
const ABSENT_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// The only `KnowledgeBaseType` a content may be uploaded into, which the operation's own
/// documentation states ("You can only upload content to a knowledge base of type
/// CUSTOM").
const TYPE_CUSTOM: &str = "CUSTOM";

/// Media types from the modelled `ContentType` catalog
/// (`smithy_query(query="shape",shape="ContentType")`:
/// `^(text/(plain|html|csv))|(application/(pdf|vnd\.openxmlformats-officedocument\.wordprocessingml\.document))|(application/x\.wisdom-json;source=(salesforce|servicenow|zendesk))$`),
/// so an asset replaced through an update is seen to carry the media type of the upload it
/// came from rather than the one the content was created with.
const TYPE_TEXT: &str = "text/plain";
const TYPE_HTML: &str = "text/html";
const TYPE_CSV: &str = "text/csv";

/// The fixed sentences this endpoint answers the two not-found classes with, written out
/// here rather than imported from the implementation so the assertions pin the wire text.
const MESSAGE_KNOWLEDGE_BASE_NOT_FOUND: &str = "Knowledge Base does not exist.";
const MESSAGE_CONTENT_NOT_FOUND: &str = "Content does not exist.";

/// The sentence an `uploadId` naming no upload of this knowledge base is refused with.
const MESSAGE_INVALID_UPLOAD_ID: &str = "Invalid uploadId";

/// The optional member of the modelled `PreconditionFailedException`, which this endpoint
/// leaves UNSET: the whole of that refusal is its 412 and its error code, so the assertions
/// pin the member's absence rather than any text.
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

    fn message(&self) -> String {
        self.string("/message")
    }

    /// Whether the body carries the error's `message` member at all — what decides the
    /// empty message a client reads.
    fn carries_message(&self) -> bool {
        self.body
            .as_object()
            .is_some_and(|body| body.contains_key(MEMBER_MESSAGE))
    }
}

/// Dispatch one already-built request and read the answer.
async fn dispatch(router: &MultiServiceRouter, request: http::Request<Bytes>) -> Reply {
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let error_type = header("x-amzn-errortype");
    let content_type = header("content-type");
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            // A non-JSON body is an asset the plane served: keep the bytes and the media
            // type it carried, so the assertions can compare both.
            json!({
                "assetBody": String::from_utf8_lossy(&bytes),
                "assetContentType": content_type,
            })
        })
    };
    Reply {
        status,
        error_type,
        body,
    }
}

/// One signed call of this endpoint, built the way an SDK builds it. Free-standing so a
/// racer can be spawned with nothing but the shared router.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Reply {
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
    dispatch(router, request).await
}

/// One `UpdateContent` call, as a racer runs it.
async fn update(
    router: Arc<MultiServiceRouter>,
    knowledge_base_id: String,
    content_id: String,
    body: Value,
) -> Reply {
    call(
        &router,
        "POST",
        &format!("/knowledgeBases/{knowledge_base_id}/contents/{content_id}"),
        Some(body),
    )
    .await
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
        // SigV4 credential scope above, which is also where an ARN's region comes from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self {
            router: Arc::new(router),
        }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        call(&self.router, method, uri, body).await
    }

    /// Follow a URL this service handed out exactly as a client does: `method` against the
    /// absolute URL, with NO credential of any kind.
    async fn follow(&self, method: &str, url: &str, headers: &[(&str, &str)], body: &[u8]) -> Reply {
        let mut builder = http::Request::builder().method(method).uri(url);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
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

    /// Start an upload of `content_type` under `knowledge_base_id` and write `asset`
    /// through the URL it hands out, so the `uploadId` names bytes this store holds.
    async fn written_upload(
        &self,
        knowledge_base_id: &str,
        content_type: &str,
        asset: &[u8],
    ) -> String {
        let reply = self
            .send(
                "POST",
                &format!("/knowledgeBases/{knowledge_base_id}/upload"),
                Some(json!({ "contentType": content_type })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"upload of {content_type}\": expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        let upload_id = reply.string("/uploadId");
        let put = self
            .follow(
                "PUT",
                &reply.string("/url"),
                &[("content-type", content_type)],
                asset,
            )
            .await;
        assert_eq!(
            put.status, 200,
            "case \"PUT the asset of {upload_id}\": expected HTTP 200, got {} with {}",
            put.status, put.body
        );
        upload_id
    }

    /// A content holding `asset`, as its create persisted it.
    async fn content(
        &self,
        knowledge_base_id: &str,
        name: &str,
        content_type: &str,
        asset: &[u8],
    ) -> Reply {
        let upload_id = self
            .written_upload(knowledge_base_id, content_type, asset)
            .await;
        let created = self
            .send(
                "POST",
                &format!("/knowledgeBases/{knowledge_base_id}/contents"),
                Some(json!({
                    "name": name,
                    "uploadId": upload_id,
                    "title": format!("the title of {name}"),
                    "metadata": { "source": "crm" },
                    "overrideLinkOutUri": "https://example.test/articles/original",
                })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case \"content {name}\": expected HTTP 200 from the create, got {} with {}",
            created.status, created.body
        );
        created
    }

    async fn get_content(&self, knowledge_base_id: &str, content_label: &str) -> Reply {
        self.send(
            "GET",
            &format!("/knowledgeBases/{knowledge_base_id}/contents/{content_label}"),
            None,
        )
        .await
    }

    async fn update_content(
        &self,
        knowledge_base_id: &str,
        content_label: &str,
        body: Value,
    ) -> Reply {
        self.send(
            "POST",
            &format!("/knowledgeBases/{knowledge_base_id}/contents/{content_label}"),
            Some(body),
        )
        .await
    }
}

/// One row of the update table: what the request names, and what the record must report
/// afterwards.
struct Row {
    case: &'static str,
    /// The body, minus the `revisionId` the test fills in from what it last read.
    body: Value,
    /// The `title` the record must carry after the update.
    title: &'static str,
    /// The `linkOutUri` it must carry — `None` for the member cleared or never set.
    link_out_uri: Option<&'static str>,
    /// The `metadata` it must carry.
    metadata: Value,
    /// The media type and asset it must hold, for a row that replaces them.
    asset: Option<(&'static str, &'static str)>,
}

/// The updates one content is walked through, in order: each row names a different subset
/// of the members the operation accepts, including one that names only `revisionId` and
/// one that replaces the asset.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a title alone",
            body: json!({ "title": "A retitled article" }),
            title: "A retitled article",
            link_out_uri: Some("https://example.test/articles/original"),
            metadata: json!({ "source": "crm" }),
            asset: None,
        },
        Row {
            case: "the metadata map, which replaces the stored one",
            body: json!({ "metadata": { "locale": "en-GB", "version": "7" } }),
            title: "A retitled article",
            link_out_uri: Some("https://example.test/articles/original"),
            metadata: json!({ "locale": "en-GB", "version": "7" }),
            asset: None,
        },
        Row {
            case: "an empty metadata map, which clears it",
            body: json!({ "metadata": {} }),
            title: "A retitled article",
            link_out_uri: Some("https://example.test/articles/original"),
            metadata: json!({}),
            asset: None,
        },
        Row {
            case: "overrideLinkOutUri, which replaces the stored one",
            body: json!({ "overrideLinkOutUri": "https://example.test/articles/second" }),
            title: "A retitled article",
            link_out_uri: Some("https://example.test/articles/second"),
            metadata: json!({}),
            asset: None,
        },
        Row {
            case: "removeOverrideLinkOutUri, which clears it",
            body: json!({ "removeOverrideLinkOutUri": true }),
            title: "A retitled article",
            link_out_uri: None,
            metadata: json!({}),
            asset: None,
        },
        Row {
            case: "a request naming nothing but its revision",
            body: json!({}),
            title: "A retitled article",
            link_out_uri: None,
            metadata: json!({}),
            asset: None,
        },
        Row {
            case: "an uploadId, which replaces the asset and its media type",
            body: json!({ "uploadId": "<filled in by the test>" }),
            title: "A retitled article",
            link_out_uri: None,
            metadata: json!({}),
            asset: Some((TYPE_HTML, "<p>the replacement asset</p>")),
        },
        Row {
            case: "every member at once, over the replaced asset",
            body: json!({
                "title": "The final title",
                "metadata": { "source": "servicenow" },
                "overrideLinkOutUri": "https://example.test/articles/final",
                "uploadId": "<filled in by the test>",
            }),
            title: "The final title",
            link_out_uri: Some("https://example.test/articles/final"),
            metadata: json!({ "source": "servicenow" }),
            asset: Some((TYPE_CSV, "a,b\n1,2\n")),
        },
    ]
}

/// RULE: an update writes the stored content — the members it names, and only those — and
/// answers out of that one record, so a later `GetContent` reports exactly what it
/// answered; every write that lands moves the content to a new `revisionId`, and that new
/// revision is the one the next write has to name.
#[tokio::test]
async fn an_update_writes_the_members_it_names_and_moves_the_content_to_a_new_revision() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.knowledge_base("update-kb").await;
    let created = emulator
        .content(&knowledge_base_id, "runbook", TYPE_TEXT, b"the first asset")
        .await;
    let content_id = created.string("/content/contentId");
    let content_arn = created.string("/content/contentArn");

    // Past the span the create's ingestion is owed over, so the status an update answers
    // and the one a read answers are comparable member for member.
    tokio::time::sleep(state::CONTENT_ACTIVATION_DELAY + Duration::from_millis(50)).await;

    let mut revision = created.string("/content/revisionId");
    let mut seen: BTreeSet<String> = BTreeSet::new();
    seen.insert(revision.clone());

    for row in rows() {
        let mut body = row.body.clone();
        // A row that replaces the asset needs a receipt this service issued for THIS
        // knowledge base, so the upload is started and written here.
        if let Some((content_type, asset)) = row.asset {
            let upload_id = emulator
                .written_upload(&knowledge_base_id, content_type, asset.as_bytes())
                .await;
            body["uploadId"] = json!(upload_id);
        }
        body["revisionId"] = json!(revision);

        let updated = emulator
            .update_content(&knowledge_base_id, &content_id, body)
            .await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected HTTP 200, got {} with {}",
            row.case, updated.status, updated.body
        );

        let reported = updated.object("/content");
        assert_eq!(
            reported.get("title").and_then(Value::as_str),
            Some(row.title),
            "case {:?}: expected title {:?}, got {}",
            row.case,
            row.title,
            updated.body
        );
        assert_eq!(
            reported.get("linkOutUri").and_then(Value::as_str),
            row.link_out_uri,
            "case {:?}: expected linkOutUri {:?}, got {}",
            row.case,
            row.link_out_uri,
            updated.body
        );
        assert_eq!(
            reported.get("metadata").cloned().unwrap_or(json!({})),
            row.metadata,
            "case {:?}: expected metadata {}, got {}",
            row.case,
            row.metadata,
            updated.body
        );
        // The members an update never names keep the values the create gave them.
        assert_eq!(
            reported.get("name").and_then(Value::as_str),
            Some("runbook"),
            "case {:?}: the name must survive an update, got {}",
            row.case,
            updated.body
        );
        assert_eq!(
            reported.get("contentArn").and_then(Value::as_str),
            Some(content_arn.as_str()),
            "case {:?}: the ARN must survive an update, got {}",
            row.case,
            updated.body
        );
        if let Some((content_type, _)) = row.asset {
            assert_eq!(
                reported.get("contentType").and_then(Value::as_str),
                Some(content_type),
                "case {:?}: a replaced asset must report the media type of the upload it \
                 came from, got {}",
                row.case,
                updated.body
            );
        }

        // Every write that lands is a NEW revision, and no revision is ever handed out
        // twice.
        let next = updated.string("/content/revisionId");
        assert_ne!(
            next, revision,
            "case {:?}: a write that lands must move the content to a new revisionId, got {}",
            row.case, updated.body
        );
        assert!(
            seen.insert(next.clone()),
            "case {:?}: a revisionId must not be handed out twice, got {next:?} again",
            row.case
        );

        // The answer is the stored record: a read reports it member for member.
        let read = emulator.get_content(&knowledge_base_id, &content_id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected HTTP 200 from the read, got {} with {}",
            row.case, read.status, read.body
        );
        assert_eq!(
            read.object("/content"),
            reported,
            "case {:?}: the read must report what the update answered, got {}",
            row.case,
            read.body
        );

        // A replaced asset is what the URL now serves, so the record and the bytes behind
        // it cannot drift.
        if let Some((content_type, asset)) = row.asset {
            let followed = emulator
                .follow("GET", &read.string("/content/url"), &[], b"")
                .await;
            assert_eq!(
                followed.status, 200,
                "case {:?}: the asset URL must be followable, got {} with {}",
                row.case, followed.status, followed.body
            );
            assert_eq!(
                followed.string("/assetBody"),
                asset,
                "case {:?}: the asset must be the bytes the named upload received, got {}",
                row.case,
                followed.body
            );
            assert_eq!(
                followed.string("/assetContentType"),
                content_type,
                "case {:?}: the asset must carry the upload's media type, got {}",
                row.case,
                followed.body
            );
        }

        revision = next;
    }

    // The ARN form of the content label addresses the same record, so an update is not
    // tied to the bare id.
    let by_arn = emulator
        .update_content(
            &knowledge_base_id,
            &content_arn.replace('/', "%2F"),
            json!({ "title": "Addressed by ARN", "revisionId": revision }),
        )
        .await;
    assert_eq!(
        by_arn.status, 200,
        "case \"the ARN label form\": expected HTTP 200, got {} with {}",
        by_arn.status, by_arn.body
    );
    assert_eq!(
        by_arn.string("/content/contentId"),
        content_id,
        "case \"the ARN label form\": expected the same content, got {}",
        by_arn.body
    );
}

/// RULE: `revisionId` is an optimistic lock — a request naming the revision the content
/// carries is applied, one naming a revision it no longer carries is refused HTTP 412
/// `PreconditionFailedException` and writes nothing, and one naming no revision asks for no
/// precondition at all.
#[tokio::test]
async fn a_request_naming_a_superseded_revision_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.knowledge_base("precondition-kb").await;
    let created = emulator
        .content(&knowledge_base_id, "policy", TYPE_TEXT, b"the first asset")
        .await;
    let content_id = created.string("/content/contentId");
    let first_revision = created.string("/content/revisionId");

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
    let before = emulator
        .get_content(&knowledge_base_id, &content_id)
        .await
        .object("/content");

    // Every shape of a revision the content does not carry: the one it carried before the
    // write above, a revision of this service's id form that names nothing, and a value of
    // another form entirely.
    for (case, named) in [
        ("the revision the content carried before the write", first_revision.clone()),
        ("a revision of this service's id form naming nothing", ABSENT_UUID.to_string()),
        ("a revision of no form this service mints", "not-a-revision".to_string()),
    ] {
        let refused = emulator
            .update_content(
                &knowledge_base_id,
                &content_id,
                json!({ "title": "A title the loser wanted", "revisionId": named }),
            )
            .await;
        assert_eq!(
            refused.status, 412,
            "case {case:?}: expected HTTP 412, got {} with {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            "PreconditionFailedException",
            "case {case:?}: expected PreconditionFailedException, got {:?} with {}",
            refused.code(),
            refused.body
        );
        assert!(
            !refused.carries_message(),
            "case {case:?}: the refusal must carry no {MEMBER_MESSAGE} member, got {:?} in {}",
            refused.message(),
            refused.body
        );
        let after = emulator
            .get_content(&knowledge_base_id, &content_id)
            .await
            .object("/content");
        assert_eq!(
            after, before,
            "case {case:?}: a refused write must leave the record as it was, got {after:?}"
        );
    }

    // The CURRENT revision is accepted, and a request naming no revision at all asks for
    // no precondition.
    let with_current = emulator
        .update_content(
            &knowledge_base_id,
            &content_id,
            json!({ "title": "Named the current revision", "revisionId": current_revision }),
        )
        .await;
    assert_eq!(
        with_current.status, 200,
        "case \"naming the current revision\": expected HTTP 200, got {} with {}",
        with_current.status, with_current.body
    );
    let without = emulator
        .update_content(
            &knowledge_base_id,
            &content_id,
            json!({ "title": "Named no revision" }),
        )
        .await;
    assert_eq!(
        without.status, 200,
        "case \"naming no revision\": expected HTTP 200, got {} with {}",
        without.status, without.body
    );
    assert_eq!(
        without.string("/content/title"),
        "Named no revision",
        "case \"naming no revision\": the write must be applied, got {}",
        without.body
    );
}

/// RULE: of several writers naming ONE revision exactly one wins — it answers 200 with the
/// next revision — and every other is refused 412, so the record holds exactly one of the
/// writes. Asserted at several widths, with the callers in flight together.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_writers_naming_one_revision_have_exactly_one_winner() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.knowledge_base("race-kb").await;
    let created = emulator
        .content(&knowledge_base_id, "contended", TYPE_TEXT, b"the first asset")
        .await;
    let content_id = created.string("/content/contentId");
    let mut revision = created.string("/content/revisionId");

    for width in [2usize, 3, 5] {
        let case = format!("{width} writers naming one revision");
        let racers: Vec<_> = (0..width)
            .map(|caller| {
                tokio::spawn(update(
                    Arc::clone(&emulator.router),
                    knowledge_base_id.clone(),
                    content_id.clone(),
                    json!({
                        "title": format!("written by caller {caller}"),
                        "revisionId": revision,
                    }),
                ))
            })
            .collect();

        let mut winners = Vec::new();
        let mut refusals = 0;
        for (caller, racer) in racers.into_iter().enumerate() {
            let answer = racer.await.expect("the update ran to completion");
            match answer.status {
                200 => winners.push((caller, answer)),
                412 => {
                    assert_eq!(
                        answer.code(),
                        "PreconditionFailedException",
                        "case {case:?}, caller {caller}: a loser must be refused \
                         PreconditionFailedException, got {:?} with {}",
                        answer.code(),
                        answer.body
                    );
                    assert!(
                        !answer.carries_message(),
                        "case {case:?}, caller {caller}: the refusal must carry no \
                         {MEMBER_MESSAGE} member, got {:?} in {}",
                        answer.message(),
                        answer.body
                    );
                    refusals += 1;
                }
                status => panic!(
                    "case {case:?}, caller {caller}: expected HTTP 200 or 412, got {status} \
                     with {}",
                    answer.body
                ),
            }
        }
        assert_eq!(
            winners.len(),
            1,
            "case {case:?}: expected exactly one winner, got {} winners and {refusals} refusals",
            winners.len()
        );
        assert_eq!(
            refusals,
            width - 1,
            "case {case:?}: expected every other writer to be refused, got {refusals}"
        );

        // The record holds the winner's write, under the revision the winner answered
        // with — which is a new one, so the losers' revision really is superseded.
        let (caller, winner) = winners.remove(0);
        let read = emulator.get_content(&knowledge_base_id, &content_id).await;
        assert_eq!(
            read.string("/content/title"),
            format!("written by caller {caller}"),
            "case {case:?}: the record must hold the winner's write, got {}",
            read.body
        );
        let next = winner.string("/content/revisionId");
        assert_ne!(
            next, revision,
            "case {case:?}: the winner must move the content to a new revisionId, got {}",
            winner.body
        );
        assert_eq!(
            read.string("/content/revisionId"),
            next,
            "case {case:?}: the stored revision must be the one the winner answered with, got {}",
            read.body
        );
        revision = next;
    }
}

/// One refusal row: the case, the knowledge-base label, the content label, the body, the
/// status and the error code the answer must carry, and the message where this endpoint
/// fixes it.
type RefusalRow = (
    &'static str,
    String,
    String,
    Value,
    u16,
    &'static str,
    Option<&'static str>,
);

/// RULE: a label the route names and this account holds nothing under is the modelled
/// not-found, and a request the service cannot apply — both halves of one paired member,
/// or an `uploadId` it did not issue — is the modelled input failure. Every one of them
/// leaves the record as it was.
#[tokio::test]
async fn an_update_this_service_cannot_apply_is_refused_and_leaves_the_record_as_it_was() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.knowledge_base("refusal-kb").await;
    let other_kb_id = emulator.knowledge_base("refusal-other-kb").await;
    let created = emulator
        .content(&knowledge_base_id, "guarded", TYPE_TEXT, b"the first asset")
        .await;
    let content_id = created.string("/content/contentId");
    let elsewhere = emulator
        .content(&other_kb_id, "elsewhere", TYPE_TEXT, b"another asset")
        .await;
    let elsewhere_id = elsewhere.string("/content/contentId");
    // An upload of the OTHER knowledge base: a receipt this service issued, but not one
    // the content below may be finalized from.
    let foreign_upload = emulator
        .written_upload(&other_kb_id, TYPE_HTML, b"not this knowledge base's asset")
        .await;

    let before = emulator
        .get_content(&knowledge_base_id, &content_id)
        .await
        .object("/content");

    // Each row: the case, the knowledge-base label, the content label, the body, the
    // status and the error code the answer must carry, and the message where this endpoint
    // fixes it.
    let rows: Vec<RefusalRow> = vec![
        (
            "a knowledge base this account does not hold",
            ABSENT_UUID.to_string(),
            content_id.clone(),
            json!({ "title": "never written" }),
            404,
            "ResourceNotFoundException",
            Some(MESSAGE_KNOWLEDGE_BASE_NOT_FOUND),
        ),
        (
            "a content this knowledge base does not hold",
            knowledge_base_id.clone(),
            ABSENT_UUID.to_string(),
            json!({ "title": "never written" }),
            404,
            "ResourceNotFoundException",
            Some(MESSAGE_CONTENT_NOT_FOUND),
        ),
        (
            "a content of ANOTHER knowledge base",
            knowledge_base_id.clone(),
            elsewhere_id.clone(),
            json!({ "title": "never written" }),
            404,
            "ResourceNotFoundException",
            Some(MESSAGE_CONTENT_NOT_FOUND),
        ),
        (
            "both halves of the overrideLinkOutUri pair",
            knowledge_base_id.clone(),
            content_id.clone(),
            json!({
                "overrideLinkOutUri": "https://example.test/articles/both",
                "removeOverrideLinkOutUri": true,
            }),
            400,
            "ValidationException",
            Some("Cannot specify both overrideLinkOutUri and removeOverrideLinkOutUri."),
        ),
        (
            "an uploadId this service never issued",
            knowledge_base_id.clone(),
            content_id.clone(),
            json!({ "uploadId": ABSENT_UUID }),
            400,
            "ValidationException",
            Some(MESSAGE_INVALID_UPLOAD_ID),
        ),
        (
            "an uploadId of another knowledge base",
            knowledge_base_id.clone(),
            content_id.clone(),
            json!({ "uploadId": foreign_upload.clone() }),
            400,
            "ValidationException",
            Some(MESSAGE_INVALID_UPLOAD_ID),
        ),
    ];

    for (case, knowledge_base_label, content_label, body, status, code, message) in rows {
        let refused = emulator
            .update_content(&knowledge_base_label, &content_label, body)
            .await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            code,
            "case {case:?}: expected {code}, got {:?} with {}",
            refused.code(),
            refused.body
        );
        if let Some(message) = message {
            assert_eq!(
                refused.message(),
                message,
                "case {case:?}: unexpected message in {}",
                refused.body
            );
        }
        let after = emulator
            .get_content(&knowledge_base_id, &content_id)
            .await
            .object("/content");
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must leave the record as it was, got {after:?}"
        );
    }

    // The content of the other knowledge base is untouched by every refusal above: a
    // request addressed across two knowledge bases writes to neither.
    let elsewhere_after = emulator.get_content(&other_kb_id, &elsewhere_id).await;
    assert_eq!(
        elsewhere_after.object("/content").get("title"),
        elsewhere.object("/content").get("title"),
        "case \"the content of the other knowledge base\": it must be untouched, got {}",
        elsewhere_after.body
    );
}
