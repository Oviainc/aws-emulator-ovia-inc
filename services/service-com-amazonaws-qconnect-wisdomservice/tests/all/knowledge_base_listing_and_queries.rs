// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the three reporting paths this change implemented
//! (`ListKnowledgeBases`, `SearchQuickResponses`, `QueryAssistant`) and for the source a
//! knowledge base of a sourced type cannot be created without.
//!
//! Every assertion goes over the wire: a request is built, dispatched through the same
//! [`MultiServiceRouter`] the production emulator mounts, and only the HTTP status, the
//! `x-amzn-errortype` header and the JSON body are inspected. Nothing reaches into the
//! service's state struct, so a response that is not computed from persisted state cannot
//! pass.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! * a knowledge base whose `knowledgeBaseType` names a source outside the service
//!   (`EXTERNAL`, `MANAGED` — the model documents `sourceConfiguration` as the source of
//!   exactly those) cannot be created without one: that create is the modelled
//!   `ValidationException` (HTTP 400) naming the type, while a create of any other
//!   modelled type needs none and a sourced create that carries one succeeds and reports
//!   the source it was given;
//! * `ListKnowledgeBases` reports every stored knowledge base exactly once at any page
//!   size, each summary agreeing with what `GetKnowledgeBase` answers for the same record,
//!   hands back no token on the last page, keeps the required
//!   `knowledgeBaseSummaries` member on an account holding none, follows a delete, and
//!   refuses a `nextToken` it never minted and a `maxResults` outside the published range;
//! * a reporting path scoped to a parent resolves that parent FIRST: `SearchQuickResponses`
//!   against a knowledge base this account does not hold, and `QueryAssistant` against an
//!   assistant it does not hold (or a session another assistant owns), is the modelled
//!   `ResourceNotFoundException` (HTTP 404) with that family's sentence, while a parent it
//!   does hold answers 200 with the required list member present.
//!
//! The harness below is this suite's own, as each suite in this crate carries its own:
//! sharing one would make a neighbouring suite's helpers this suite's contract.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_qconnect_wisdomservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for.
const REGION: &str = "us-west-2";

/// A UUID the modelled `UuidOrArn` pattern admits and no create ever mints. The all-zero
/// id is NOT one: it names the system-managed session every assistant is provisioned
/// with.
const ABSENT_UUID: &str = "ffffffff-ffff-ffff-ffff-ffffffffffff";

/// Every value of the modelled `KnowledgeBaseType` enum
/// (`smithy_query(query="shape",shape="KnowledgeBaseType")`), so the tables below span the
/// whole catalog rather than the one type a recording used.
const KNOWLEDGE_BASE_TYPES: [&str; 5] = [
    "CUSTOM",
    "EXTERNAL",
    "QUICK_RESPONSES",
    "MESSAGE_TEMPLATES",
    "MANAGED",
];

/// The types whose content comes from a source outside the service: the model documents
/// `sourceConfiguration` as "The source of the knowledge base content. Only set this
/// argument for EXTERNAL or Managed knowledge bases"
/// (`smithy_query(query="doc",shape="CreateKnowledgeBaseRequest",member="sourceConfiguration")`).
const SOURCED_TYPES: [&str; 2] = ["EXTERNAL", "MANAGED"];

/// The published bounds of `maxResults` on `ListKnowledgeBases`
/// (`smithy_query(query="operation",operation="ListKnowledgeBases")`: `range 1..100`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// The service's fixed sentence for each refusal these tests reach, written out here
/// rather than imported from `state.rs` so the assertion states the text instead of
/// restating the implementation's constant.
const MESSAGE_ASSISTANT_NOT_FOUND: &str = "Assistant does not exist.";
const MESSAGE_KNOWLEDGE_BASE_NOT_FOUND: &str = "Knowledge Base does not exist.";
const MESSAGE_SESSION_NOT_FOUND: &str = "Session does not exist.";

/// One wire answer, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: String,
    body: Value,
}

impl Reply {
    fn field(&self, pointer: &str) -> Option<&Value> {
        self.body.pointer(pointer).filter(|value| !value.is_null())
    }

    fn string(&self, pointer: &str) -> Option<String> {
        self.field(pointer)
            .and_then(Value::as_str)
            .map(str::to_string)
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
            .uri(uri)
            .header("host", format!("wisdom.{REGION}.amazonaws.com"))
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

    async fn create_knowledge_base(&self, body: Value) -> Reply {
        self.send("POST", "/knowledgeBases", Some(body)).await
    }

    async fn get_knowledge_base(&self, label: &str) -> Reply {
        self.send("GET", &format!("/knowledgeBases/{}", encode(label)), None)
            .await
    }

    async fn list_knowledge_bases(&self, query: &str) -> Reply {
        self.send("GET", &format!("/knowledgeBases{query}"), None)
            .await
    }

    async fn create_assistant(&self, name: &str) -> Reply {
        self.send(
            "POST",
            "/assistants",
            Some(json!({ "name": name, "type": "AGENT" })),
        )
        .await
    }

    async fn create_session(&self, assistant_label: &str, name: &str) -> Reply {
        self.send(
            "POST",
            &format!("/assistants/{}/sessions", encode(assistant_label)),
            Some(json!({ "name": name })),
        )
        .await
    }

    async fn search_quick_responses(&self, knowledge_base_label: &str, body: Value) -> Reply {
        self.send(
            "POST",
            &format!(
                "/knowledgeBases/{}/search/quickResponses",
                encode(knowledge_base_label)
            ),
            Some(body),
        )
        .await
    }

    async fn query_assistant(&self, assistant_label: &str, body: Value) -> Reply {
        self.send(
            "POST",
            &format!("/assistants/{}/query", encode(assistant_label)),
            Some(body),
        )
        .await
    }
}

/// A `UuidOrArn` label as it travels in a URL: an ARN's slashes are percent-encoded, or
/// they would be path separators naming another route.
fn encode(label: &str) -> String {
    label.replace('/', "%2F")
}

/// The `sourceConfiguration` a knowledge base of `knowledge_base_type` cannot be created
/// without, or `None` for a type whose content lives in the service itself.
///
/// One union member per source (`smithy_query(query="shape",shape="SourceConfiguration")`:
/// `appIntegrations`, `managedSourceConfiguration`), each carrying the member its own shape
/// marks `@required` (`appIntegrationArn`;`urlConfiguration` on the web crawler).
fn source_configuration(knowledge_base_type: &str) -> Option<Value> {
    match knowledge_base_type {
        "EXTERNAL" => Some(json!({
            "appIntegrations": {
                "appIntegrationArn": format!(
                    "arn:aws:app-integrations:{REGION}:{account}:data-integration/kb-source",
                    account = engine::EMULATOR_ACCOUNT_ID
                )
            }
        })),
        "MANAGED" => Some(json!({
            "managedSourceConfiguration": {
                "webCrawlerConfiguration": {
                    "urlConfiguration": { "seedUrls": [{ "url": "https://example.test/docs" }] }
                }
            }
        })),
        _ => None,
    }
}

/// A `CreateKnowledgeBase` body for one name and type, carrying the source that type
/// needs — see [`source_configuration`].
fn create_body(name: &str, knowledge_base_type: &str) -> Value {
    let mut body = json!({ "name": name, "knowledgeBaseType": knowledge_base_type });
    if let Some(source) = source_configuration(knowledge_base_type) {
        body["sourceConfiguration"] = source;
    }
    body
}

/// Create one knowledge base and answer its `(id, arn)`.
async fn given_knowledge_base(emulator: &Emulator, case: &str, name: &str, kind: &str) -> (String, String) {
    let created = emulator.create_knowledge_base(create_body(name, kind)).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the knowledge base must be created, got {} with {}",
        created.status, created.body
    );
    (
        created
            .string("/knowledgeBase/knowledgeBaseId")
            .unwrap_or_else(|| panic!("case {case:?}: no knowledgeBaseId in {}", created.body)),
        created
            .string("/knowledgeBase/knowledgeBaseArn")
            .unwrap_or_else(|| panic!("case {case:?}: no knowledgeBaseArn in {}", created.body)),
    )
}

/// The label forms that name no resource of this account, for a family's `{id}` label:
/// a well-formed id nothing holds, this account's ARN for it, and ARNs of another account
/// and another partition carrying a LIVE id.
fn absent_labels(family: &str, live_id: &str) -> Vec<(String, String)> {
    vec![
        ("a well-formed absent id".to_string(), ABSENT_UUID.to_string()),
        (
            "this account's arn for an absent id".to_string(),
            format!(
                "arn:aws:wisdom:{REGION}:{account}:{family}/{ABSENT_UUID}",
                account = engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        (
            "another account's arn for a live id".to_string(),
            format!("arn:aws:wisdom:{REGION}:123456789012:{family}/{live_id}"),
        ),
        (
            "another partition's arn for a live id".to_string(),
            format!(
                "arn:aws-cn:wisdom:{REGION}:{account}:{family}/{live_id}",
                account = engine::EMULATOR_ACCOUNT_ID
            ),
        ),
    ]
}

/// A knowledge base of a sourced type cannot be created without a `sourceConfiguration`,
/// every other modelled type needs none, and a sourced create that carries one is stored
/// with the source it was given.
#[tokio::test]
async fn a_sourced_knowledge_base_type_cannot_be_created_without_its_source() {
    let emulator = Emulator::new();

    for knowledge_base_type in KNOWLEDGE_BASE_TYPES {
        let sourced = SOURCED_TYPES.contains(&knowledge_base_type);
        let case = format!("{knowledge_base_type} with no sourceConfiguration");

        let bare = emulator
            .create_knowledge_base(json!({
                "name": format!("sourceless-{knowledge_base_type}").replace('_', "-"),
                "knowledgeBaseType": knowledge_base_type,
            }))
            .await;
        if sourced {
            assert_eq!(
                (bare.status, bare.error_type.as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected HTTP 400 ValidationException, got {} {:?} ({})",
                bare.status,
                bare.error_type,
                bare.body
            );
            assert_eq!(
                bare.string("/message").as_deref(),
                Some(
                    format!(
                        "KnowledgeBase of type {knowledge_base_type} must have at least one \
                         SourceConfiguration."
                    )
                    .as_str()
                ),
                "case {case:?}: expected the refusal to state the type the request named, \
                 got {:?}",
                bare.string("/message")
            );
            assert_eq!(
                bare.field("/knowledgeBase"),
                None,
                "case {case:?}: a refused create reports no knowledge base, got {}",
                bare.body
            );
        } else {
            assert_eq!(
                bare.status, 200,
                "case {case:?}: a type that holds its own content needs no source, got {} \
                 ({})",
                bare.status, bare.body
            );
        }

        // And a create of the same type that DOES carry the source its type names is
        // stored with it, whichever union member that source uses.
        let case = format!("{knowledge_base_type} with its sourceConfiguration");
        let name = format!("sourced-{knowledge_base_type}").replace('_', "-");
        let mut body = json!({ "name": name, "knowledgeBaseType": knowledge_base_type });
        let source = source_configuration(knowledge_base_type)
            .unwrap_or_else(|| json!({ "appIntegrations": { "appIntegrationArn":
                format!("arn:aws:app-integrations:{REGION}:{account}:data-integration/any",
                    account = engine::EMULATOR_ACCOUNT_ID) } }));
        body["sourceConfiguration"] = source.clone();
        let created = emulator.create_knowledge_base(body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200, got {} ({})",
            created.status, created.body
        );
        let id = created
            .string("/knowledgeBase/knowledgeBaseId")
            .unwrap_or_else(|| panic!("case {case:?}: no knowledgeBaseId in {}", created.body));
        let read_back = emulator.get_knowledge_base(&id).await;
        assert_eq!(
            read_back.field("/knowledgeBase/sourceConfiguration"),
            Some(&source),
            "case {case:?}: expected the stored source to read back, got {}",
            read_back.body
        );
    }
}

/// `ListKnowledgeBases` reports every stored knowledge base exactly once at any page size,
/// each summary agreeing with the record `GetKnowledgeBase` answers for it, and hands back
/// no token on the last page.
#[tokio::test]
async fn list_knowledge_bases_pages_over_every_stored_record() {
    let emulator = Emulator::new();

    // An account holding none still reports the required member.
    let empty = emulator.list_knowledge_bases("").await;
    assert_eq!(
        empty.status, 200,
        "case \"empty account\": expected HTTP 200, got {} ({})",
        empty.status, empty.body
    );
    assert_eq!(
        empty.body.pointer("/knowledgeBaseSummaries"),
        Some(&json!([])),
        "case \"empty account\": expected the required member as an empty list, got {}",
        empty.body
    );
    assert_eq!(
        empty.field("/nextToken"),
        None,
        "case \"empty account\": nothing follows an empty page, got {}",
        empty.body
    );

    // Seven records, spanning the modelled types, so a page size that does not divide the
    // count is covered too.
    let mut created: Vec<String> = Vec::new();
    for index in 0..7 {
        let kind = KNOWLEDGE_BASE_TYPES[index % KNOWLEDGE_BASE_TYPES.len()];
        let (id, _) = given_knowledge_base(
            &emulator,
            "listing setup",
            &format!("listed-kb-{index}"),
            kind,
        )
        .await;
        created.push(id);
    }
    created.sort();

    // Page sizes differing along what the rule generalises over: both published bounds of
    // `maxResults`, a size that divides the count, one that does not, one larger than the
    // count, and no `maxResults` at all.
    for case in ["", "?maxResults=1", "?maxResults=3", "?maxResults=7", "?maxResults=100"] {
        let mut seen: Vec<String> = Vec::new();
        let mut query = case.to_string();
        loop {
            let page = emulator.list_knowledge_bases(&query).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected HTTP 200 for a page, got {} ({})",
                page.status, page.body
            );
            let summaries = page
                .body
                .pointer("/knowledgeBaseSummaries")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: no knowledgeBaseSummaries in {}", page.body));
            for summary in &summaries {
                let id = summary["knowledgeBaseId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: no knowledgeBaseId in {summary}"))
                    .to_string();
                // Every member the summary reports is the stored record's, so a list page
                // and a read of the same knowledge base cannot disagree.
                let read_back = emulator.get_knowledge_base(&id).await;
                for member in [
                    "knowledgeBaseId",
                    "knowledgeBaseArn",
                    "name",
                    "knowledgeBaseType",
                    "status",
                    "tags",
                ] {
                    assert_eq!(
                        summary.get(member),
                        read_back.body.pointer(&format!("/knowledgeBase/{member}")),
                        "case {case:?}: the listed {member} of {id} must be the stored one, \
                         got {:?} for {:?}",
                        summary.get(member),
                        read_back.body.pointer(&format!("/knowledgeBase/{member}"))
                    );
                }
                seen.push(id);
            }
            match page.string("/nextToken") {
                None => break,
                Some(token) => {
                    assert!(
                        !summaries.is_empty(),
                        "case {case:?}: a page that hands back a token must report records, \
                         got {}",
                        page.body
                    );
                    query = if case.is_empty() {
                        format!("?nextToken={token}")
                    } else {
                        format!("{case}&nextToken={token}")
                    };
                }
            }
        }
        seen.sort();
        assert_eq!(
            seen, created,
            "case {case:?}: expected every stored knowledge base exactly once, got {seen:?} \
             for {created:?}"
        );
    }

    // The listing follows the store: a deleted record is gone from the page.
    let deleted = created[0].clone();
    let gone = emulator
        .send("DELETE", &format!("/knowledgeBases/{deleted}"), None)
        .await;
    assert_eq!(
        gone.status, 204,
        "case \"delete then list\": expected HTTP 204 for the delete, got {} ({})",
        gone.status, gone.body
    );
    let after = emulator.list_knowledge_bases("").await;
    let listed: Vec<String> = after
        .body
        .pointer("/knowledgeBaseSummaries")
        .and_then(Value::as_array)
        .map(|summaries| {
            summaries
                .iter()
                .filter_map(|summary| summary["knowledgeBaseId"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !listed.contains(&deleted) && listed.len() == created.len() - 1,
        "case \"delete then list\": expected the deleted {deleted} to be gone and the rest \
         kept, got {listed:?}"
    );
}

/// A `maxResults` outside the published range, and a `nextToken` this service never
/// minted, are refused rather than read as "the whole page" or "from the beginning".
#[tokio::test]
async fn list_knowledge_bases_refuses_a_page_it_cannot_serve() {
    let emulator = Emulator::new();
    given_knowledge_base(&emulator, "refusal setup", "paged-kb", "CUSTOM").await;

    for (case, query) in [
        ("maxResults one below the published minimum", format!("?maxResults={}", MAX_RESULTS_MIN - 1)),
        ("maxResults one past the published maximum", format!("?maxResults={}", MAX_RESULTS_MAX + 1)),
        ("a nextToken this service never minted", "?nextToken=not-a-token".to_string()),
    ] {
        let reply = emulator.list_knowledge_bases(&query).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} ({})",
            reply.status, reply.body
        );
    }

    // Both published bounds are served.
    for (case, query) in [
        ("maxResults at the published minimum", format!("?maxResults={MAX_RESULTS_MIN}")),
        ("maxResults at the published maximum", format!("?maxResults={MAX_RESULTS_MAX}")),
    ] {
        let reply = emulator.list_knowledge_bases(&query).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} ({})",
            reply.status, reply.body
        );
    }
}

/// `SearchQuickResponses` resolves the knowledge base its path names before it reports
/// anything: a parent this account does not hold is the modelled not-found, and one it
/// does hold answers the required `results` member.
#[tokio::test]
async fn search_quick_responses_resolves_its_parent_knowledge_base() {
    let emulator = Emulator::new();
    let (live_id, live_arn) =
        given_knowledge_base(&emulator, "search setup", "searched-kb", "QUICK_RESPONSES").await;

    // The search expression varies along what the operation accepts: a name query, a
    // filter, and an ordering (`smithy_query(query="shape",shape="QuickResponseSearchExpression")`).
    let expressions: Vec<(&str, Value)> = vec![
        ("an empty expression", json!({})),
        (
            "a name query",
            json!({ "queries": [{ "name": "name", "values": ["greeting"], "operator": "CONTAINS" }] }),
        ),
        (
            "a filter and an ordering",
            json!({
                "filters": [{ "name": "name", "operator": "EQUALS", "values": ["greeting"] }],
                "orderOnField": { "name": "name", "order": "ASC" }
            }),
        ),
    ];

    for (case, expression) in &expressions {
        for (addressing, label) in [("by id", &live_id), ("by arn", &live_arn)] {
            let reply = emulator
                .search_quick_responses(label, json!({ "searchExpression": expression }))
                .await;
            assert_eq!(
                reply.status, 200,
                "case {case:?} ({addressing}): expected HTTP 200 for a live parent, got {} \
                 ({})",
                reply.status, reply.body
            );
            assert_eq!(
                reply.body.pointer("/results"),
                Some(&json!([])),
                "case {case:?} ({addressing}): expected the required results member for a \
                 knowledge base holding no quick response, got {}",
                reply.body
            );
            assert_eq!(
                reply.field("/nextToken"),
                None,
                "case {case:?} ({addressing}): nothing follows an empty page, got {}",
                reply.body
            );
        }
    }

    for (case, label) in absent_labels("knowledge-base", &live_id) {
        let reply = emulator
            .search_quick_responses(&label, json!({ "searchExpression": {} }))
            .await;
        assert_eq!(
            (reply.status, reply.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException for {label}, got {} \
             {:?} ({})",
            reply.status,
            reply.error_type,
            reply.body
        );
        assert_eq!(
            reply.string("/message").as_deref(),
            Some(MESSAGE_KNOWLEDGE_BASE_NOT_FOUND),
            "case {case:?}: expected the family's fixed sentence, got {:?}",
            reply.string("/message")
        );
    }
}

/// `QueryAssistant` resolves the assistant its path names, and the session its body names
/// under THAT assistant, before it reports anything.
#[tokio::test]
async fn query_assistant_resolves_its_assistant_and_session() {
    let emulator = Emulator::new();

    let first = emulator.create_assistant("queried-assistant").await;
    let assistant_id = first
        .string("/assistant/assistantId")
        .unwrap_or_else(|| panic!("setup: no assistantId in {}", first.body));
    let assistant_arn = first
        .string("/assistant/assistantArn")
        .unwrap_or_else(|| panic!("setup: no assistantArn in {}", first.body));
    let session = emulator.create_session(&assistant_id, "queried-session").await;
    let session_id = session
        .string("/session/sessionId")
        .unwrap_or_else(|| panic!("setup: no sessionId in {}", session.body));

    // A second assistant with its own session, so a cross-parent session is covered.
    let other = emulator.create_assistant("other-assistant").await;
    let other_id = other
        .string("/assistant/assistantId")
        .unwrap_or_else(|| panic!("setup: no assistantId in {}", other.body));
    let other_session = emulator.create_session(&other_id, "other-session").await;
    let other_session_id = other_session
        .string("/session/sessionId")
        .unwrap_or_else(|| panic!("setup: no sessionId in {}", other_session.body));

    // Bodies differing along what the query accepts: nothing, a query text at and under
    // the published maximum (`queryText` is `len ..512`), a session, and a query
    // condition.
    let bodies: Vec<(&str, Value)> = vec![
        ("no query at all", json!({})),
        ("a short query text", json!({ "queryText": "how do I reset a password" })),
        ("a query text at the published maximum", json!({ "queryText": "q".repeat(512) })),
        (
            "a query text and the session it belongs to",
            json!({ "queryText": "hello", "sessionId": session_id }),
        ),
        (
            "a query condition",
            json!({
                "queryText": "hello",
                "queryCondition": [{ "single": { "field": "RESULT_TYPE", "comparator": "EQUALS", "value": "KNOWLEDGE_CONTENT" } }]
            }),
        ),
    ];

    for (case, body) in &bodies {
        for (addressing, label) in [("by id", &assistant_id), ("by arn", &assistant_arn)] {
            let reply = emulator.query_assistant(label, body.clone()).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?} ({addressing}): expected HTTP 200 for a live assistant, got {} \
                 ({})",
                reply.status, reply.body
            );
            assert_eq!(
                reply.body.pointer("/results"),
                Some(&json!([])),
                "case {case:?} ({addressing}): expected the required results member for an \
                 assistant holding no content, got {}",
                reply.body
            );
        }
    }

    for (case, label) in absent_labels("assistant", &assistant_id) {
        let reply = emulator
            .query_assistant(&label, json!({ "queryText": "hello" }))
            .await;
        assert_eq!(
            (reply.status, reply.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException for {label}, got {} \
             {:?} ({})",
            reply.status,
            reply.error_type,
            reply.body
        );
        assert_eq!(
            reply.string("/message").as_deref(),
            Some(MESSAGE_ASSISTANT_NOT_FOUND),
            "case {case:?}: expected the family's fixed sentence, got {:?}",
            reply.string("/message")
        );
    }

    // A session this assistant does not own names nothing here — whether nothing holds it
    // or another assistant does.
    for (case, named_session) in [
        ("a session id nothing holds", ABSENT_UUID.to_string()),
        ("a session another assistant owns", other_session_id),
    ] {
        let reply = emulator
            .query_assistant(
                &assistant_id,
                json!({ "queryText": "hello", "sessionId": named_session }),
            )
            .await;
        assert_eq!(
            (reply.status, reply.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException, got {} {:?} ({})",
            reply.status,
            reply.error_type,
            reply.body
        );
        assert_eq!(
            reply.string("/message").as_deref(),
            Some(MESSAGE_SESSION_NOT_FOUND),
            "case {case:?}: expected the session family's fixed sentence, got {:?}",
            reply.string("/message")
        );
    }
}
