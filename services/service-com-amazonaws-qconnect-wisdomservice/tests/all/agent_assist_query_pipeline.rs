// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the agent-assist query pipeline: the chain a client composes this
//! endpoint with — assistant, knowledge base, association, content upload, session,
//! recommendations and finally `QueryAssistant` — driven over the wire through the same
//! [`MultiServiceRouter`] the production emulator mounts.
//!
//! The rules under test, each asserted over a TABLE of documents and queries rather than
//! one recorded request:
//!
//! * a query REPORTS the ingested contents of the knowledge bases the named assistant is
//!   associated with that carry at least one term of the query, each result carrying that
//!   content's own reference (the ids and ARNs a read of the content answers with), its
//!   title and a window of the text the query matched in;
//! * a term is matched case-insensitively and term by term, so neither the caller's letter
//!   case nor the punctuation between the terms has to be reproduced by the document, and
//!   what is searched is the content's `name`, its `title` and its stored asset — the asset
//!   only when the asset IS text, so a binary document type is matched on its naming alone;
//! * the results are RANKED by how much of the query a content carries, so a content
//!   carrying every term outranks one carrying half of them;
//! * a content of a knowledge base this assistant is NOT associated with is never a result,
//!   and neither is a content whose ingestion has not finished;
//! * `maxResults`/`nextToken` walk the matches, the last page carries no token, and a token
//!   this service never minted is the modelled `ValidationException` (HTTP 400);
//! * the query WRITES NOTHING: two identical queries report the same contents with the same
//!   scores, and the content they name reads back unchanged.
//!
//! Every assertion goes over the wire and nothing reaches into the service's state struct,
//! so a response that is not computed from persisted state cannot pass.
//!
//! The harness below is this suite's own, as each suite in this crate carries its own.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_qconnect_wisdomservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for.
const REGION: &str = "us-west-2";

/// The endpoint host those requests address.
const ENDPOINT_HOST: &str = "wisdom.us-west-2.amazonaws.com";

/// The `QueryResultType` a knowledge-content hit carries
/// (`smithy_query(query="shape",shape="QueryResultType")`).
const RESULT_TYPE_KNOWLEDGE_CONTENT: &str = "KNOWLEDGE_CONTENT";

/// The terminal `ContentStatus` of an ingested content, and the in-progress one it passes
/// through (`smithy_query(query="shape",shape="ContentStatus")`).
const CONTENT_STATUS_ACTIVE: &str = "ACTIVE";
const CONTENT_STATUS_CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";

/// How long a poll waits for an ingestion to finish before giving up. Generous: the span
/// the service owes the transition is its own, and this only has to outlast it.
const ACTIVATION_TIMEOUT: Duration = Duration::from_secs(10);

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

    fn array(&self, pointer: &str) -> Vec<Value> {
        self.body
            .pointer(pointer)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    fn object(&self, pointer: &str) -> Map<String, Value> {
        self.body
            .pointer(pointer)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    }
}

/// An emulator with just this service mounted on a fresh engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what this suite is about; the router still routes by the
        // SigV4 credential scope below, which is also where an ARN's region comes from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        let builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{ENDPOINT_HOST}{uri}"))
            .header("host", ENDPOINT_HOST)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDEXAMPLE/20260101/{REGION}/wisdom/aws4_request, \
                     SignedHeaders=host, Signature=not-verified-in-this-suite"
                ),
            );
        let request = match &body {
            Some(value) => builder
                .header("content-type", "application/json")
                .body(Bytes::from(
                    serde_json::to_vec(value).expect("serialize the request body"),
                ))
                .expect("build a request with a JSON body"),
            None => builder.body(Bytes::new()).expect("build a bodyless request"),
        };
        self.dispatch(request).await
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
            .expect("build the upload request");
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
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    /// An assistant, as its create persisted it: `(id, arn)`.
    async fn assistant(&self, name: &str) -> (String, String) {
        let reply = self
            .send(
                "POST",
                "/assistants",
                Some(json!({ "name": name, "type": "AGENT" })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"assistant {name}\": expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        (
            reply.string("/assistant/assistantId"),
            reply.string("/assistant/assistantArn"),
        )
    }

    /// A `CUSTOM` knowledge base, as its create persisted it: `(id, arn)`.
    async fn knowledge_base(&self, name: &str) -> (String, String) {
        let reply = self
            .send(
                "POST",
                "/knowledgeBases",
                Some(json!({ "name": name, "knowledgeBaseType": "CUSTOM" })),
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

    /// Associate one knowledge base with one assistant and answer the association id.
    async fn associate(&self, assistant_id: &str, knowledge_base_id: &str) -> String {
        let reply = self
            .send(
                "POST",
                &format!("/assistants/{assistant_id}/associations"),
                Some(json!({
                    "associationType": "KNOWLEDGE_BASE",
                    "association": { "knowledgeBaseId": knowledge_base_id },
                })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"associate {knowledge_base_id}\": expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        reply.string("/assistantAssociation/assistantAssociationId")
    }

    async fn list_associations(&self, assistant_label: &str, query: &str) -> Reply {
        self.send(
            "GET",
            &format!(
                "/assistants/{}/associations{query}",
                label(assistant_label)
            ),
            None,
        )
        .await
    }

    /// Start an upload, PUT the asset through the URL it hands out, and finalize it into a
    /// content: the whole ingestion hop of the chain. Answers the content id.
    async fn ingest(&self, knowledge_base_id: &str, document: &Document) -> String {
        let started = self
            .send(
                "POST",
                &format!("/knowledgeBases/{knowledge_base_id}/upload"),
                Some(json!({ "contentType": document.content_type })),
            )
            .await;
        assert_eq!(
            started.status, 200,
            "case {:?}: expected HTTP 200 from the upload start, got {} with {}",
            document.case, started.status, started.body
        );
        let put = self
            .follow(
                "PUT",
                &started.string("/url"),
                &[("content-type", document.content_type)],
                document.asset.as_bytes(),
            )
            .await;
        assert_eq!(
            put.status, 200,
            "case {:?}: expected HTTP 200 from the asset PUT, got {} with {}",
            document.case, put.status, put.body
        );

        let mut body = json!({ "name": document.name, "uploadId": started.string("/uploadId") });
        if let Some(title) = document.title {
            body["title"] = json!(title);
        }
        let created = self
            .send(
                "POST",
                &format!("/knowledgeBases/{knowledge_base_id}/contents"),
                Some(body),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected HTTP 200 from the create, got {} with {}",
            document.case, created.status, created.body
        );
        created.string("/content/contentId")
    }

    async fn get_content(&self, knowledge_base_id: &str, content_id: &str) -> Reply {
        self.send(
            "GET",
            &format!("/knowledgeBases/{knowledge_base_id}/contents/{content_id}"),
            None,
        )
        .await
    }

    /// Poll the content until its ingestion has finished, as a client does, and answer the
    /// `ContentData` the last read reported.
    async fn active_content(&self, knowledge_base_id: &str, content_id: &str) -> Map<String, Value> {
        let deadline = Instant::now() + ACTIVATION_TIMEOUT;
        loop {
            let reply = self.get_content(knowledge_base_id, content_id).await;
            assert_eq!(
                reply.status, 200,
                "case \"poll {content_id}\": expected HTTP 200, got {} with {}",
                reply.status, reply.body
            );
            let status = reply.string("/content/status");
            if status == CONTENT_STATUS_ACTIVE {
                return reply.object("/content");
            }
            assert_eq!(
                status, CONTENT_STATUS_CREATE_IN_PROGRESS,
                "case \"poll {content_id}\": expected the ingestion statuses, got {status:?}"
            );
            assert!(
                Instant::now() < deadline,
                "case \"poll {content_id}\": the content never reached {CONTENT_STATUS_ACTIVE}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    async fn create_session(&self, assistant_id: &str, name: &str) -> Reply {
        self.send(
            "POST",
            &format!("/assistants/{assistant_id}/sessions"),
            Some(json!({ "name": name })),
        )
        .await
    }

    async fn get_session(&self, assistant_id: &str, session_id: &str) -> Reply {
        self.send(
            "GET",
            &format!("/assistants/{assistant_id}/sessions/{session_id}"),
            None,
        )
        .await
    }

    async fn get_recommendations(&self, assistant_id: &str, session_id: &str) -> Reply {
        self.send(
            "GET",
            &format!("/assistants/{assistant_id}/sessions/{session_id}/recommendations"),
            None,
        )
        .await
    }

    async fn query(&self, assistant_label: &str, body: Value) -> Reply {
        self.send(
            "POST",
            &format!("/assistants/{}/query", label(assistant_label)),
            Some(body),
        )
        .await
    }
}

/// A `UuidOrArn` label as it travels in a URL: an ARN's slashes are percent-encoded, or
/// they would be path separators naming another route.
fn label(value: &str) -> String {
    value.replace('/', "%2F")
}

/// One document of the corpus every query in this suite searches: how it is named, the
/// media type its upload is started for, the title the create gives it, and the asset its
/// upload carries.
struct Document {
    case: &'static str,
    name: &'static str,
    content_type: &'static str,
    title: Option<&'static str>,
    asset: &'static str,
}

/// The documents the associated knowledge base holds.
///
/// The media types are drawn from the whole modelled `ContentType` catalog
/// (`smithy_query(query="shape",shape="ContentType")`:
/// `^(text/(plain|html|csv))|(application/(pdf|vnd\.openxmlformats-officedocument\.wordprocessingml\.document))|(application/x\.wisdom-json;source=(salesforce|servicenow|zendesk))$`),
/// including the binary document types whose bytes are not text, and the rows differ along
/// what the matching rule generalises over: which field carries the term (asset, title,
/// name), the letter case it is written in, and whether the asset is text at all.
fn documents() -> Vec<Document> {
    vec![
        Document {
            case: "a plain-text article whose asset carries the marker",
            name: "escalation-runbook",
            content_type: "text/plain",
            title: Some("Escalation runbook"),
            asset: "To escalate, page the duty manager before issuing a refund. \
                    Marker: zulu-7781 confirms this revision.",
        },
        Document {
            case: "an HTML article whose asset carries the term in another case",
            name: "billing-faq",
            content_type: "text/html",
            title: None,
            asset: "<p>REFUND requests are processed within seven days.</p>",
        },
        Document {
            case: "a CSV table whose NAME carries the term",
            name: "refund-codes",
            content_type: "text/csv",
            title: Some("Codes and meanings"),
            asset: "code,meaning\nRF1,issued\nRF2,declined\n",
        },
        Document {
            case: "a JSON-source article whose asset carries the term",
            name: "zendesk-import",
            content_type: "application/x.wisdom-json;source=zendesk",
            title: None,
            asset: "{\"body\":\"password reset steps for a locked account\"}",
        },
        Document {
            case: "a PDF whose bytes are not text",
            name: "policy-2026",
            content_type: "application/pdf",
            title: Some("Policy 2026"),
            asset: "%PDF-1.7 binary stream mentioning refund",
        },
    ]
}

/// One query of the table: the text a caller sends and the documents it must report, by
/// their case names.
struct Query {
    case: &'static str,
    text: &'static str,
    expected: &'static [&'static str],
}

/// The queries, spanning what the rule generalises over: one term, several terms, the
/// letter case, the punctuation between terms, which field carries the term, a term only a
/// binary asset carries, and a query naming nothing the corpus holds.
fn queries() -> Vec<Query> {
    vec![
        Query {
            case: "one term three documents carry",
            text: "refund",
            expected: &[
                "a plain-text article whose asset carries the marker",
                "an HTML article whose asset carries the term in another case",
                "a CSV table whose NAME carries the term",
            ],
        },
        Query {
            case: "several terms, punctuation and all",
            text: "Marker: zulu-7781",
            expected: &["a plain-text article whose asset carries the marker"],
        },
        Query {
            case: "the same terms in another case",
            text: "ZULU 7781",
            expected: &["a plain-text article whose asset carries the marker"],
        },
        Query {
            case: "a term only a JSON-source asset carries",
            text: "password",
            expected: &["a JSON-source article whose asset carries the term"],
        },
        Query {
            case: "a term only a binary asset carries",
            text: "binary stream",
            expected: &[],
        },
        Query {
            case: "a term the binary document's own naming carries",
            text: "policy",
            expected: &["a PDF whose bytes are not text"],
        },
        Query {
            case: "a term nothing in the corpus carries",
            text: "xyzzy-nothing-here",
            expected: &[],
        },
        Query {
            case: "no term at all",
            text: "",
            expected: &[],
        },
    ]
}

/// The whole corpus, ingested into one knowledge base associated with one assistant, plus
/// a SECOND knowledge base that is NOT associated and holds a document carrying every term
/// — so a result from it would be a result from data this assistant cannot answer out of.
struct Pipeline {
    emulator: Emulator,
    assistant_id: String,
    assistant_arn: String,
    knowledge_base_id: String,
    /// The created content of each document, by case name: `(contentId, ContentData)`.
    contents: Vec<(&'static str, String, Map<String, Value>)>,
}

impl Pipeline {
    async fn new() -> Self {
        let emulator = Emulator::new();
        let (assistant_id, assistant_arn) = emulator.assistant("agent-assist-assistant").await;
        let (knowledge_base_id, _) = emulator.knowledge_base("agent-assist-kb").await;
        emulator.associate(&assistant_id, &knowledge_base_id).await;

        let mut contents = Vec::new();
        for document in documents() {
            let content_id = emulator.ingest(&knowledge_base_id, &document).await;
            let data = emulator
                .active_content(&knowledge_base_id, &content_id)
                .await;
            contents.push((document.case, content_id, data));
        }

        // The knowledge base nothing associated this assistant with.
        let (other_knowledge_base_id, _) = emulator.knowledge_base("unassociated-kb").await;
        emulator
            .ingest(
                &other_knowledge_base_id,
                &Document {
                    case: "a document of an unassociated knowledge base",
                    name: "unassociated-refund-policy",
                    content_type: "text/plain",
                    title: Some("Refund policy"),
                    asset: "refund, zulu-7781, password, policy — every term the queries name.",
                },
            )
            .await;

        Self {
            emulator,
            assistant_id,
            assistant_arn,
            knowledge_base_id,
            contents,
        }
    }

    /// The content id the document with this case name was created as.
    fn content_id(&self, case: &str) -> &str {
        self.contents
            .iter()
            .find(|(name, _, _)| *name == case)
            .map(|(_, id, _)| id.as_str())
            .unwrap_or_else(|| panic!("setup: no content for case {case:?}"))
    }

    /// What a read of that content answers with.
    fn content_data(&self, case: &str) -> &Map<String, Value> {
        self.contents
            .iter()
            .find(|(name, _, _)| *name == case)
            .map(|(_, _, data)| data)
            .unwrap_or_else(|| panic!("setup: no content for case {case:?}"))
    }
}

/// The `contentId`s a query answer reports, in the order it reported them.
fn reported_ids(reply: &Reply) -> Vec<String> {
    reply
        .array("/results")
        .iter()
        .map(|result| {
            result
                .pointer("/document/contentReference/contentId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Whether `value` has the form of the modelled `Uuid` shape
/// (`smithy_query(query="shape",shape="Uuid")`:
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`).
fn is_modelled_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(length, group)| {
                group.len() == *length
                    && group
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
}

/// RULE: a query reports the ingested contents of the knowledge bases the assistant is
/// associated with that carry a term of the query — matched case-insensitively, term by
/// term, over the content's name, title and text asset — and nothing else.
#[tokio::test]
async fn a_query_reports_the_associated_contents_carrying_its_terms() {
    let pipeline = Pipeline::new().await;

    for query in queries() {
        for (addressing, assistant_label) in [
            ("by id", pipeline.assistant_id.clone()),
            ("by arn", pipeline.assistant_arn.clone()),
        ] {
            let reply = pipeline
                .emulator
                .query(&assistant_label, json!({ "queryText": query.text }))
                .await;
            assert_eq!(
                reply.status, 200,
                "case {:?} ({addressing}): expected HTTP 200, got {} with {}",
                query.case, reply.status, reply.body
            );

            let mut reported = reported_ids(&reply);
            reported.sort();
            let mut expected: Vec<String> = query
                .expected
                .iter()
                .map(|case| pipeline.content_id(case).to_string())
                .collect();
            expected.sort();
            assert_eq!(
                reported, expected,
                "case {:?} ({addressing}): expected the contents {:?}, got {}",
                query.case, query.expected, reply.body
            );
        }
    }
}

/// RULE: each result is the STORED content — its reference carries the very ids and ARNs a
/// read of that content answers with, its title is the content's title, and the excerpt is
/// a window of the text the query matched in with the matched spans highlighted against
/// that window.
#[tokio::test]
async fn a_result_carries_the_stored_content_and_the_matched_text() {
    let pipeline = Pipeline::new().await;
    let case = "a plain-text article whose asset carries the marker";
    let content_id = pipeline.content_id(case).to_string();
    let data = pipeline.content_data(case).clone();

    let reply = pipeline
        .emulator
        .query(&pipeline.assistant_id, json!({ "queryText": "zulu-7781" }))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {}",
        reply.status, reply.body
    );
    let results = reply.array("/results");
    assert_eq!(
        results.len(),
        1,
        "case {case:?}: expected exactly the one matching content, got {}",
        reply.body
    );
    let result = &results[0];

    let result_id = result
        .pointer("/resultId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        is_modelled_uuid(result_id),
        "case {case:?}: expected a modelled Uuid resultId, got {result_id:?}"
    );

    let reference = result
        .pointer("/document/contentReference")
        .cloned()
        .unwrap_or(Value::Null);
    for (member, expected) in [
        ("contentId", content_id.clone()),
        (
            "contentArn",
            data.get("contentArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
        ("knowledgeBaseId", pipeline.knowledge_base_id.clone()),
        (
            "knowledgeBaseArn",
            data.get("knowledgeBaseArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
    ] {
        let actual = reference
            .pointer(&format!("/{member}"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the stored {member} {expected:?} in the reference, got \
             {actual:?}"
        );
    }

    let title = result
        .pointer("/document/title/text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        title,
        data.get("title").and_then(Value::as_str).unwrap_or_default(),
        "case {case:?}: expected the stored title, got {title:?}"
    );

    // The excerpt is of the asset the upload wrote, and the highlight offsets index THAT
    // text: the characters they name are the term the query asked for.
    let excerpt = result
        .pointer("/document/excerpt/text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        excerpt.to_lowercase().contains("zulu"),
        "case {case:?}: expected an excerpt of the matched asset text, got {excerpt:?}"
    );
    let highlights = result
        .pointer("/document/excerpt/highlights")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        highlights.len(),
        2,
        "case {case:?}: expected one highlight per term of the query, got {highlights:?}"
    );
    let characters: Vec<char> = excerpt.chars().collect();
    let highlighted: Vec<String> = highlights
        .iter()
        .map(|highlight| {
            let begin = highlight
                .pointer("/beginOffsetInclusive")
                .and_then(Value::as_u64)
                .unwrap_or_default() as usize;
            let end = highlight
                .pointer("/endOffsetExclusive")
                .and_then(Value::as_u64)
                .unwrap_or_default() as usize;
            characters[begin.min(characters.len())..end.min(characters.len())]
                .iter()
                .collect::<String>()
                .to_lowercase()
        })
        .collect();
    assert_eq!(
        highlighted,
        vec!["zulu".to_string(), "7781".to_string()],
        "case {case:?}: expected the highlights to name the query's terms in the excerpt, got \
         {highlighted:?}"
    );

    // The current API reports the same hit under `data`/`type`, out of the same record.
    assert_eq!(
        result
            .pointer("/type")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        RESULT_TYPE_KNOWLEDGE_CONTENT,
        "case {case:?}: expected the knowledge-content result type, got {result}"
    );
    assert_eq!(
        result.pointer("/data/reference/contentReference/contentId"),
        Some(&json!(content_id)),
        "case {case:?}: expected the same content under data.reference, got {result}"
    );
    assert_eq!(
        result.pointer("/data/details/contentData/textData/excerpt/text"),
        result.pointer("/document/excerpt/text"),
        "case {case:?}: expected one excerpt under both members, got {result}"
    );
    assert_eq!(
        result.pointer("/data/details/contentData/rankingData/relevanceScore"),
        result.pointer("/relevanceScore"),
        "case {case:?}: expected one score under both members, got {result}"
    );
}

/// RULE: the results are ranked by how much of the query a content carries, so a content
/// carrying every term outranks one carrying only some of them.
#[tokio::test]
async fn the_results_are_ranked_by_how_much_of_the_query_a_content_carries() {
    let pipeline = Pipeline::new().await;
    let whole = pipeline.content_id("a plain-text article whose asset carries the marker");

    // "refund" is carried by three documents; "zulu" only by the first, so that one
    // carries both terms and the others half of them.
    let reply = pipeline
        .emulator
        .query(
            &pipeline.assistant_id,
            json!({ "queryText": "refund zulu-7781" }),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case \"ranked query\": expected HTTP 200, got {} with {}",
        reply.status, reply.body
    );
    let results = reply.array("/results");
    assert_eq!(
        results.len(),
        3,
        "case \"ranked query\": expected the three contents carrying a term, got {}",
        reply.body
    );
    let scores: Vec<f64> = results
        .iter()
        .map(|result| {
            result
                .pointer("/relevanceScore")
                .and_then(Value::as_f64)
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(
        reported_ids(&reply).first().map(String::as_str),
        Some(whole),
        "case \"ranked query\": expected the content carrying every term first, got {}",
        reply.body
    );
    for (position, window) in scores.windows(2).enumerate() {
        assert!(
            window[0] >= window[1],
            "case \"ranked query\": expected a non-increasing ranking, got {scores:?} at \
             position {position}"
        );
    }
    assert!(
        scores[0] > scores[2],
        "case \"ranked query\": expected the whole-query match to outrank a partial one, got \
         {scores:?}"
    );
}

/// RULE: `maxResults`/`nextToken` walk the matches once each, the last page carries no
/// token, and a token this service never minted is refused.
#[tokio::test]
async fn the_matches_are_walked_by_max_results_and_next_token() {
    let pipeline = Pipeline::new().await;
    let mut expected: Vec<String> = [
        "a plain-text article whose asset carries the marker",
        "an HTML article whose asset carries the term in another case",
        "a CSV table whose NAME carries the term",
    ]
    .iter()
    .map(|case| pipeline.content_id(case).to_string())
    .collect();
    expected.sort();

    // Every page size the model allows up to the match count, plus the published maximum
    // (`maxResults` is `range 1..100`).
    for page_size in [1usize, 2, 3, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..=expected.len() {
            let mut body = json!({ "queryText": "refund", "maxResults": page_size });
            if let Some(token) = &token {
                body["nextToken"] = json!(token);
            }
            let reply = pipeline.emulator.query(&pipeline.assistant_id, body).await;
            assert_eq!(
                reply.status, 200,
                "case \"page size {page_size}\": expected HTTP 200, got {} with {}",
                reply.status, reply.body
            );
            let page = reported_ids(&reply);
            assert!(
                page.len() <= page_size,
                "case \"page size {page_size}\": expected at most {page_size} results, got {}",
                reply.body
            );
            walked.extend(page);
            token = reply
                .body
                .pointer("/nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case \"page size {page_size}\": expected the walk to end, got a token {token:?}"
        );
        walked.sort();
        assert_eq!(
            walked, expected,
            "case \"page size {page_size}\": expected every match once, got {walked:?}"
        );
    }

    let refused = pipeline
        .emulator
        .query(
            &pipeline.assistant_id,
            json!({ "queryText": "refund", "nextToken": "not-a-token-this-service-minted" }),
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_type.as_str()),
        (400, "ValidationException"),
        "case \"a token nothing minted\": expected HTTP 400 ValidationException, got {} {:?} \
         ({})",
        refused.status,
        refused.error_type,
        refused.body
    );
}

/// RULE: the whole chain answers — association, ingestion, session, recommendations and
/// query — and the query writes nothing: repeated queries report the same contents and
/// scores, and the content they name reads back unchanged.
#[tokio::test]
async fn the_chained_pipeline_answers_every_hop_and_the_query_writes_nothing() {
    let pipeline = Pipeline::new().await;
    let case = "a plain-text article whose asset carries the marker";
    let content_id = pipeline.content_id(case).to_string();

    // The association hop: the listing reflects what the create persisted.
    let listed = pipeline
        .emulator
        .list_associations(&pipeline.assistant_id, "")
        .await;
    assert_eq!(
        listed.status, 200,
        "case \"list associations\": expected HTTP 200, got {} with {}",
        listed.status, listed.body
    );
    let listed_knowledge_bases: Vec<String> = listed
        .array("/assistantAssociationSummaries")
        .iter()
        .map(|summary| {
            summary
                .pointer("/associationData/knowledgeBaseAssociation/knowledgeBaseId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        listed_knowledge_bases,
        vec![pipeline.knowledge_base_id.clone()],
        "case \"list associations\": expected the associated knowledge base, got {}",
        listed.body
    );

    // The session hops.
    let session = pipeline
        .emulator
        .create_session(&pipeline.assistant_id, "agent-assist-session")
        .await;
    assert_eq!(
        session.status, 200,
        "case \"create session\": expected HTTP 200, got {} with {}",
        session.status, session.body
    );
    let session_id = session.string("/session/sessionId");
    let read = pipeline
        .emulator
        .get_session(&pipeline.assistant_id, &session_id)
        .await;
    assert_eq!(
        (read.status, read.string("/session/sessionId")),
        (200, session_id.clone()),
        "case \"get session\": expected the session just created, got {} with {}",
        read.status,
        read.body
    );
    let recommendations = pipeline
        .emulator
        .get_recommendations(&pipeline.assistant_id, &session_id)
        .await;
    assert_eq!(
        recommendations.status, 200,
        "case \"get recommendations\": expected HTTP 200, got {} with {}",
        recommendations.status, recommendations.body
    );
    assert!(
        recommendations
            .body
            .pointer("/recommendations")
            .is_some_and(Value::is_array),
        "case \"get recommendations\": expected the required recommendations list, got {}",
        recommendations.body
    );

    // The query hop, twice: a read reports the same thing to the next caller.
    let body = json!({ "queryText": "zulu-7781", "sessionId": session_id });
    let first = pipeline
        .emulator
        .query(&pipeline.assistant_id, body.clone())
        .await;
    let second = pipeline.emulator.query(&pipeline.assistant_id, body).await;
    assert_eq!(
        (reported_ids(&first), reported_ids(&second)),
        (vec![content_id.clone()], vec![content_id.clone()]),
        "case \"repeated query\": expected the same content both times, got {} then {}",
        first.body,
        second.body
    );
    assert_eq!(
        first.body.pointer("/results/0/relevanceScore"),
        second.body.pointer("/results/0/relevanceScore"),
        "case \"repeated query\": expected the same score both times, got {} then {}",
        first.body,
        second.body
    );
    assert_eq!(
        first.body.pointer("/results/0/document/excerpt"),
        second.body.pointer("/results/0/document/excerpt"),
        "case \"repeated query\": expected the same excerpt both times, got {} then {}",
        first.body,
        second.body
    );

    let after = pipeline
        .emulator
        .get_content(&pipeline.knowledge_base_id, &content_id)
        .await;
    assert_eq!(
        (after.status, after.object("/content")),
        (200, pipeline.content_data(case).clone()),
        "case \"the content after the queries\": expected the record untouched, got {} with {}",
        after.status,
        after.body
    );
}

/// RULE: a query bigger than this endpoint searches over is REFUSED rather than served
/// slowly — the model's own character ceiling on `queryText`, and this operation's ceiling
/// on the distinct terms it scans for; a query AT either ceiling is still served.
#[tokio::test]
async fn a_query_past_what_the_endpoint_searches_over_is_refused() {
    let pipeline = Pipeline::new().await;

    // The ceilings, written out here rather than imported so the assertion states the limit
    // instead of restating the implementation's constant. The character ceiling is the
    // model's (`QueryText` is `len ..512`,
    // `smithy_query(query="shape",shape="QueryText")`).
    const MAX_QUERY_TEXT_CHARS: usize = 512;
    const MAX_QUERY_TERMS: usize = 64;

    let terms_of = |count: usize| -> String {
        (0..count)
            .map(|index| format!("t{index}"))
            .collect::<Vec<String>>()
            .join(" ")
    };
    // Each published bound, and one past it. A query past the model's CHARACTER ceiling is
    // refused where the request body is read into the input shape, which this endpoint
    // answers with its own `BadRequestException` (see `src/service.rs`); a query past this
    // operation's TERM ceiling is the operation's own modelled `ValidationException`.
    let cases: Vec<(&str, String, u16, Option<&str>)> = vec![
        (
            "a query at the character ceiling",
            "a".repeat(MAX_QUERY_TEXT_CHARS),
            200,
            None,
        ),
        (
            "a query one character past it",
            "a".repeat(MAX_QUERY_TEXT_CHARS + 1),
            400,
            Some("BadRequestException"),
        ),
        (
            "a query at the term ceiling",
            terms_of(MAX_QUERY_TERMS),
            200,
            None,
        ),
        (
            "a query one term past it",
            terms_of(MAX_QUERY_TERMS + 1),
            400,
            Some("ValidationException"),
        ),
    ];
    for (case, query, expected, code) in cases {
        let reply = pipeline
            .emulator
            .query(&pipeline.assistant_id, json!({ "queryText": query }))
            .await;
        assert_eq!(
            reply.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with {}",
            reply.status, reply.body
        );
        if let Some(code) = code {
            assert_eq!(
                reply.error_type, code,
                "case {case:?}: expected the error code {code}, got {:?} ({})",
                reply.error_type, reply.body
            );
        }
    }
}

/// RULE: a content whose ingestion has not finished is not searchable — a document the
/// service has not taken in yet cannot be found by a query.
#[tokio::test]
async fn a_content_still_being_ingested_is_not_searchable() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("ingestion-gate-assistant").await;
    let (knowledge_base_id, _) = emulator.knowledge_base("ingestion-gate-kb").await;
    emulator.associate(&assistant_id, &knowledge_base_id).await;

    let document = Document {
        case: "a content read while still being ingested",
        name: "in-progress-article",
        content_type: "text/plain",
        title: Some("In progress"),
        asset: "The marker quebec-4412 is only in this asset.",
    };
    let content_id = emulator.ingest(&knowledge_base_id, &document).await;

    // The create answers in progress, so the status is read FIRST and the query is judged
    // against what that read reported: whichever side of the transition this lands on, the
    // query and the status agree.
    let status = emulator
        .get_content(&knowledge_base_id, &content_id)
        .await
        .string("/content/status");
    let reply = emulator
        .query(&assistant_id, json!({ "queryText": "quebec-4412" }))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: expected HTTP 200, got {} with {}",
        document.case, reply.status, reply.body
    );
    if status == CONTENT_STATUS_CREATE_IN_PROGRESS {
        assert_eq!(
            reported_ids(&reply),
            Vec::<String>::new(),
            "case {:?}: expected no result while the content is {status}, got {}",
            document.case,
            reply.body
        );
    }

    // Once the ingestion the clock owed has finished, the same query finds it.
    emulator.active_content(&knowledge_base_id, &content_id).await;
    let found = emulator
        .query(&assistant_id, json!({ "queryText": "quebec-4412" }))
        .await;
    assert_eq!(
        reported_ids(&found),
        vec![content_id],
        "case {:?}: expected the ingested content once it is {CONTENT_STATUS_ACTIVE}, got {}",
        document.case,
        found.body
    );
}

/// RULE: a `queryCondition` selects on the result type, the one field the model's
/// `QueryConditionFieldName` catalog carries: a condition naming the knowledge-content type
/// keeps these results, and one naming any other type selects none of them.
#[tokio::test]
async fn a_query_condition_selects_on_the_result_type() {
    let pipeline = Pipeline::new().await;
    let expected = pipeline
        .content_id("a plain-text article whose asset carries the marker")
        .to_string();

    // Values from the modelled `QueryResultType` catalog
    // (`smithy_query(query="shape",shape="QueryResultType")`), including types this
    // emulator produces no result of.
    let cases: Vec<(&str, Vec<String>)> = vec![
        (RESULT_TYPE_KNOWLEDGE_CONTENT, vec![expected.clone()]),
        ("GENERATIVE_ANSWER", Vec::new()),
        ("INTENT_ANSWER", Vec::new()),
        ("NOTES", Vec::new()),
    ];
    for (result_type, expected) in cases {
        let reply = pipeline
            .emulator
            .query(
                &pipeline.assistant_id,
                json!({
                    "queryText": "zulu-7781",
                    "queryCondition": [{
                        "single": {
                            "field": "RESULT_TYPE",
                            "comparator": "EQUALS",
                            "value": result_type,
                        }
                    }],
                }),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {result_type:?}: expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        assert_eq!(
            reported_ids(&reply),
            expected,
            "case {result_type:?}: expected {expected:?}, got {}",
            reply.body
        );
    }
}
