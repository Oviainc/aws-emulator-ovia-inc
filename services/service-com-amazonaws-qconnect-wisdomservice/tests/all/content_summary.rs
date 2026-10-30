// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetContentSummary`, driven over HTTP through the same router
//! the production emulator mounts.
//!
//! The rules under test, each asserted over a TABLE of contents rather than one recorded
//! request:
//!
//! * a summary read REPORTS the stored content: every member the summary shape carries is
//!   the one the full read of the same content answers with, and the members the summary
//!   shape does not carry — the asset locator and the link-out URI — are absent from it,
//!   so the summary is the full record reduced rather than a second, independently
//!   computed answer. The table spans the media types the model admits, a content with and
//!   without a title, with and without metadata, tags and a link-out URI;
//! * either label form addresses the content: the id and the ARN of the same content
//!   answer alike, and so do the id and the ARN of its knowledge base;
//! * the read writes nothing: two reads of an unchanged content answer alike and the
//!   asset locator the full read reports is untouched by them;
//! * the status it reports is the one the clock owes: the transient create status while
//!   the ingestion is due, and the terminal one once that span has passed;
//! * a label this account holds no knowledge base under, a content id its knowledge base
//!   does not hold, and a content of ANOTHER knowledge base are all the modelled
//!   `ResourceNotFoundException` (HTTP 404), naming the requested label in the member the
//!   model provides for it.
//!
//! Everything goes over the wire, so a response that is not computed from persisted state
//! cannot pass.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_qconnect_wisdomservice::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for.
const REGION: &str = "us-west-2";

/// The endpoint host those requests address.
const ENDPOINT_HOST: &str = "wisdom.us-west-2.amazonaws.com";

/// A UUID no create ever mints, for the not-found assertions. The all-zero id is NOT
/// one: it names the system-managed session every assistant is provisioned with.
const ABSENT_UUID: &str = "ffffffff-ffff-ffff-ffff-ffffffffffff";

/// The members `ContentData` carries and `ContentSummary` does not
/// (`smithy_query(query="shape",shape="ContentSummary,ContentData")`): the asset locator
/// and the link-out URI, which a summary names no part of.
const OMITTED_MEMBERS: [&str; 3] = ["url", "urlExpiry", "linkOutUri"];

/// The asset locator members of `ContentData`, read back after a summary read to show it
/// moved nothing.
const LOCATOR_MEMBERS: [&str; 2] = ["url", "urlExpiry"];

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

    /// The error code a client branches on: the `x-amzn-errortype` header, falling back
    /// to the body's `__type` for a protocol that carries it only there.
    fn code(&self) -> String {
        if self.error_type.is_empty() {
            self.string("/__type")
        } else {
            self.error_type.clone()
        }
    }
}

/// A `UuidOrArn` label as it travels in a URL: an ARN's slashes are percent-encoded.
fn encode_label(label: &str) -> String {
    label.replace('/', "%2F")
}

/// An emulator with just this service mounted on a fresh engine, driven in-process
/// through the production router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes by the
        // SigV4 credential scope below, which is also where an ARN's region comes from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/wisdom/aws4_request, \
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
        self.dispatch(request).await
    }

    /// Follow a URL this service handed out exactly as a client does: `method` against
    /// the absolute URL, with NO credential of any kind.
    async fn follow(&self, method: &str, url: &str, headers: &[(&str, &str)], body: &[u8]) -> Reply {
        let mut builder = http::Request::builder().method(method).uri(url);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(Bytes::copy_from_slice(body))
            .expect("build request");
        self.dispatch(request).await
    }

    async fn dispatch(&self, request: http::Request<Bytes>) -> Reply {
        let response = self.router.dispatch(request).await;
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
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| json!({ "assetBody": String::from_utf8_lossy(&bytes) }))
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    /// A knowledge base of type `kind`, as its create persisted it.
    async fn knowledge_base(&self, name: &str, kind: &str) -> (String, String) {
        let reply = self
            .send(
                "POST",
                "/knowledgeBases",
                Some(json!({ "name": name, "knowledgeBaseType": kind })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"knowledge base {name}\": expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        (
            reply.string("/knowledgeBase/knowledgeBaseId"),
            reply.string("/knowledgeBase/knowledgeBaseArn"),
        )
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

    /// `GetContentSummary` for one content, addressed by the given labels.
    async fn summary(&self, knowledge_base_label: &str, content_label: &str) -> Reply {
        self.send(
            "GET",
            &format!(
                "/knowledgeBases/{}/contents/{}/summary",
                encode_label(knowledge_base_label),
                encode_label(content_label)
            ),
            None,
        )
        .await
    }

    /// `GetContent` for one content, addressed by id.
    async fn content(&self, knowledge_base_id: &str, content_id: &str) -> Reply {
        self.send(
            "GET",
            &format!("/knowledgeBases/{knowledge_base_id}/contents/{content_id}"),
            None,
        )
        .await
    }
}

/// One row of the content table: a case name, the content's name, the media type its
/// upload is started for, and the members the create leaves out or fills in.
struct Row {
    case: &'static str,
    name: &'static str,
    content_type: &'static str,
    extra: Value,
}

/// The contents under test. The media types are drawn from the whole modelled
/// `ContentType` catalog (`smithy_query(query="operation",operation="StartContentUpload")`:
/// `^(text/(plain|html|csv))|(application/(pdf|vnd\.openxmlformats-officedocument\.wordprocessingml\.document))|(application/x\.wisdom-json;source=(salesforce|servicenow|zendesk))$`),
/// including entries the issue cited none of, and the rows differ in what a create may
/// leave out: the title, the metadata, the tags and the link-out URI.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a content with every member",
            name: "full-content",
            content_type: "text/plain",
            extra: json!({
                "title": "A full content",
                "metadata": { "source": "crm", "externalVersion": "7" },
                "tags": { "team": "support" },
                "overrideLinkOutUri": "https://example.test/articles/1",
            }),
        },
        Row {
            case: "a content with no title, metadata or tags",
            name: "bare-content",
            content_type: "text/html",
            extra: json!({}),
        },
        Row {
            case: "a content with metadata but no tags",
            name: "metadata-only-content",
            content_type: "text/csv",
            extra: json!({ "metadata": { "locale": "en-GB" } }),
        },
        Row {
            case: "a content with tags but no metadata",
            name: "tagged-content",
            content_type: "application/pdf",
            extra: json!({ "tags": { "team": "billing", "tier": "gold" } }),
        },
        Row {
            case: "a content of an office media type",
            name: "office-content",
            content_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            extra: json!({ "title": "An office document" }),
        },
        Row {
            case: "a content of a connector media type",
            name: "connector-content",
            content_type: "application/x.wisdom-json;source=salesforce",
            extra: json!({ "title": "A connector record", "metadata": { "table": "kb" } }),
        },
        Row {
            case: "a content of another connector media type",
            name: "zendesk-content",
            content_type: "application/x.wisdom-json;source=zendesk",
            extra: json!({}),
        },
    ]
}

/// One created content, as the create reported it.
struct Created {
    case: &'static str,
    content_id: String,
    content_arn: String,
    data: Map<String, Value>,
}

/// Create every row's content under `knowledge_base_id`.
async fn create_contents(emulator: &Emulator, knowledge_base_id: &str) -> Vec<Created> {
    let mut created = Vec::new();
    for row in rows() {
        let asset = format!("the asset of {}", row.name).into_bytes();
        let upload_id = emulator
            .written_upload(knowledge_base_id, row.content_type, &asset)
            .await;
        let mut body = json!({ "name": row.name, "uploadId": upload_id });
        for (key, value) in row.extra.as_object().expect("the extra members") {
            body[key] = value.clone();
        }
        let reply = emulator
            .send(
                "POST",
                &format!("/knowledgeBases/{knowledge_base_id}/contents"),
                Some(body),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected HTTP 200 from the create, got {} with {}",
            row.case, reply.status, reply.body
        );
        created.push(Created {
            case: row.case,
            content_id: reply.string("/content/contentId"),
            content_arn: reply.string("/content/contentArn"),
            data: reply.object("/content"),
        });
    }
    created
}

/// RULE: a summary read reports the content the create persisted, reduced to the members
/// the summary shape carries — every one of them equal to the full read's, and the
/// members the summary shape does not carry absent.
#[tokio::test]
async fn a_summary_is_the_stored_content_without_its_asset_locator() {
    let emulator = Emulator::new();
    let (knowledge_base_id, knowledge_base_arn) =
        emulator.knowledge_base("summary-kb", "CUSTOM").await;
    let created = create_contents(&emulator, &knowledge_base_id).await;

    // Past the span the create's ingestion is owed over, so the full read and the summary
    // read are both of a content in its terminal status and the two are comparable
    // member by member.
    tokio::time::sleep(state::CONTENT_ACTIVATION_DELAY + Duration::from_millis(50)).await;

    for entry in &created {
        let case = entry.case;
        let summary_reply = emulator.summary(&knowledge_base_id, &entry.content_id).await;
        assert_eq!(
            summary_reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {}",
            summary_reply.status, summary_reply.body
        );
        let summary = summary_reply.object("/contentSummary");
        assert!(
            !summary.is_empty(),
            "case {case:?}: expected a contentSummary, got {}",
            summary_reply.body
        );

        let full_reply = emulator.content(&knowledge_base_id, &entry.content_id).await;
        assert_eq!(
            full_reply.status, 200,
            "case {case:?}: expected HTTP 200 from the full read, got {} with {}",
            full_reply.status, full_reply.body
        );
        let full = full_reply.object("/content");

        // The summary carries the full record's members minus the ones its shape does not
        // model, and nothing the full record does not carry.
        let mut expected_members: Vec<String> = full
            .keys()
            .filter(|key| !OMITTED_MEMBERS.contains(&key.as_str()))
            .cloned()
            .collect();
        expected_members.sort();
        let mut summary_members: Vec<String> = summary.keys().cloned().collect();
        summary_members.sort();
        assert_eq!(
            summary_members, expected_members,
            "case {case:?}: expected the full record's members without {OMITTED_MEMBERS:?}, \
             got {summary_members:?}"
        );
        for member in &expected_members {
            assert_eq!(
                summary.get(member),
                full.get(member),
                "case {case:?}: member {member:?} must be the stored one the full read \
                 answers with, got {:?}",
                summary.get(member)
            );
        }
        for omitted in OMITTED_MEMBERS {
            assert!(
                !summary.contains_key(omitted),
                "case {case:?}: the summary shape carries no {omitted:?}, yet the answer \
                 did: {:?}",
                summary.get(omitted)
            );
        }

        // Spot-check the identity members against what the create reported, so a summary
        // computed from something other than the stored record cannot pass.
        for member in [
            "contentId",
            "contentArn",
            "knowledgeBaseId",
            "knowledgeBaseArn",
            "name",
            "title",
            "revisionId",
            "contentType",
        ] {
            assert_eq!(
                summary.get(member),
                entry.data.get(member),
                "case {case:?}: member {member:?} must be the one the create persisted \
                 ({:?}), got {:?}",
                entry.data.get(member),
                summary.get(member)
            );
        }
        assert_eq!(
            summary.get("knowledgeBaseArn").and_then(Value::as_str),
            Some(knowledge_base_arn.as_str()),
            "case {case:?}: the summary must report its knowledge base's own ARN, got {:?}",
            summary.get("knowledgeBaseArn")
        );
        assert_eq!(
            summary.get("status").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {case:?}: after the activation span the summary reports the terminal \
             status, got {:?}",
            summary.get("status")
        );
    }
}

/// RULE: either label form addresses the same resource — a `UuidOrArn` label is the id or
/// the ARN of one stored record — and the read writes nothing, so repeated reads of an
/// unchanged content answer alike and the asset locator the full read reports is
/// untouched.
#[tokio::test]
async fn every_label_form_answers_alike_and_the_read_changes_nothing() {
    let emulator = Emulator::new();
    let (knowledge_base_id, knowledge_base_arn) =
        emulator.knowledge_base("label-form-kb", "CUSTOM").await;
    let created = create_contents(&emulator, &knowledge_base_id).await;
    tokio::time::sleep(state::CONTENT_ACTIVATION_DELAY + Duration::from_millis(50)).await;

    for entry in &created {
        let case = entry.case;
        let before = emulator.content(&knowledge_base_id, &entry.content_id).await;
        let baseline = emulator
            .summary(&knowledge_base_id, &entry.content_id)
            .await
            .object("/contentSummary");

        for (form, knowledge_base_label, content_label) in [
            (
                "both ids",
                knowledge_base_id.clone(),
                entry.content_id.clone(),
            ),
            (
                "the content by ARN",
                knowledge_base_id.clone(),
                entry.content_arn.clone(),
            ),
            (
                "the knowledge base by ARN",
                knowledge_base_arn.clone(),
                entry.content_id.clone(),
            ),
            (
                "both ARNs",
                knowledge_base_arn.clone(),
                entry.content_arn.clone(),
            ),
        ] {
            let reply = emulator.summary(&knowledge_base_label, &content_label).await;
            assert_eq!(
                reply.status, 200,
                "case \"{case} addressed by {form}\": expected HTTP 200, got {} with {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.object("/contentSummary"),
                baseline,
                "case \"{case} addressed by {form}\": every label form of one content \
                 answers alike, got {}",
                reply.body
            );
        }

        // Nothing the summary reads moved: the full read still reports the same asset
        // locator it did before.
        let after = emulator.content(&knowledge_base_id, &entry.content_id).await;
        for locator in LOCATOR_MEMBERS {
            assert_eq!(
                after.object("/content").get(locator),
                before.object("/content").get(locator),
                "case {case:?}: a summary read must leave {locator:?} as it was, got {:?}",
                after.object("/content").get(locator)
            );
        }
    }
}

/// RULE: the status a resource the service reports in a non-terminal status moves to is
/// owed by the CLOCK, and a summary read reports the status the clock owes at the instant
/// of the read, writing nothing to make it so.
#[tokio::test]
async fn the_summary_reports_the_status_the_clock_owes() {
    let emulator = Emulator::new();
    let (knowledge_base_id, _) = emulator.knowledge_base("clock-kb", "CUSTOM").await;
    let upload_id = emulator
        .written_upload(&knowledge_base_id, "text/plain", b"asset")
        .await;
    let created = emulator
        .send(
            "POST",
            &format!("/knowledgeBases/{knowledge_base_id}/contents"),
            Some(json!({ "name": "activating-content", "uploadId": upload_id })),
        )
        .await;
    let content_id = created.string("/content/contentId");

    let during = emulator.summary(&knowledge_base_id, &content_id).await;
    assert_eq!(
        during.string("/contentSummary/status"),
        "CREATE_IN_PROGRESS",
        "case \"straight after the create\": expected the transient status, got {}",
        during.body
    );

    tokio::time::sleep(state::CONTENT_ACTIVATION_DELAY + Duration::from_millis(50)).await;

    let after = emulator.summary(&knowledge_base_id, &content_id).await;
    assert_eq!(
        after.string("/contentSummary/status"),
        "ACTIVE",
        "case \"after the activation span\": expected the terminal status, got {}",
        after.body
    );
    let again = emulator.summary(&knowledge_base_id, &content_id).await;
    assert_eq!(
        again.object("/contentSummary"),
        after.object("/contentSummary"),
        "case \"a second read after the activation span\": two reads of an unchanged \
         content answer alike, got {}",
        again.body
    );
}

/// RULE: a label this account holds no such resource under is the modelled
/// `ResourceNotFoundException` (HTTP 404), stating the sentence this endpoint words for
/// that resource family and naming the requested label in the member the model provides
/// for it — and a content is reachable only through the knowledge base it lives in, so
/// another knowledge base's content is not found either.
#[tokio::test]
async fn a_label_nothing_is_held_under_is_the_modelled_not_found() {
    let emulator = Emulator::new();
    let (knowledge_base_id, knowledge_base_arn) =
        emulator.knowledge_base("not-found-kb", "CUSTOM").await;
    let (other_id, _) = emulator
        .knowledge_base("not-found-other-kb", "CUSTOM")
        .await;
    let upload_id = emulator
        .written_upload(&knowledge_base_id, "text/plain", b"asset")
        .await;
    let created = emulator
        .send(
            "POST",
            &format!("/knowledgeBases/{knowledge_base_id}/contents"),
            Some(json!({ "name": "held-content", "uploadId": upload_id })),
        )
        .await;
    let content_id = created.string("/content/contentId");
    let content_arn = created.string("/content/contentArn");
    let absent_knowledge_base_arn = format!(
        "arn:aws:wisdom:{REGION}:{}:knowledge-base/{ABSENT_UUID}",
        engine::EMULATOR_ACCOUNT_ID
    );

    let table: [(&str, String, String, &str, String); 5] = [
        (
            "a knowledge base id nothing was created under",
            ABSENT_UUID.to_string(),
            content_id.clone(),
            state::MESSAGE_KNOWLEDGE_BASE_NOT_FOUND,
            ABSENT_UUID.to_string(),
        ),
        (
            "a knowledge base ARN nothing was created under",
            absent_knowledge_base_arn.clone(),
            content_id.clone(),
            state::MESSAGE_KNOWLEDGE_BASE_NOT_FOUND,
            absent_knowledge_base_arn.clone(),
        ),
        (
            "a content id its knowledge base does not hold",
            knowledge_base_id.clone(),
            ABSENT_UUID.to_string(),
            state::MESSAGE_CONTENT_NOT_FOUND,
            ABSENT_UUID.to_string(),
        ),
        (
            "a content of another knowledge base",
            other_id.clone(),
            content_id.clone(),
            state::MESSAGE_CONTENT_NOT_FOUND,
            content_id.clone(),
        ),
        (
            "a content of another knowledge base, addressed by ARN",
            other_id.clone(),
            content_arn.clone(),
            state::MESSAGE_CONTENT_NOT_FOUND,
            content_arn.clone(),
        ),
    ];

    for (case, knowledge_base_label, content_label, message, resource_name) in table {
        let reply = emulator.summary(&knowledge_base_label, &content_label).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {} with {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?} with {}",
            reply.code(),
            reply.body
        );
        assert_eq!(
            reply.string("/message"),
            message,
            "case {case:?}: expected the sentence this endpoint words for that resource, \
             got {:?}",
            reply.string("/message")
        );
        assert_eq!(
            reply.string("/resourceName"),
            resource_name,
            "case {case:?}: the requested label travels in resourceName, got {:?}",
            reply.string("/resourceName")
        );
    }

    // The knowledge base ARN of the HOLDING base still addresses it, so the not-found
    // rows above are about the label naming nothing, not about the ARN form.
    let held = emulator.summary(&knowledge_base_arn, &content_id).await;
    assert_eq!(
        held.status, 200,
        "case \"the holding knowledge base by ARN\": expected HTTP 200, got {} with {}",
        held.status, held.body
    );
}
