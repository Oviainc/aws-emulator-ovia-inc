// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Wisdom quick-response SEARCH plane, driven over HTTP through
//! the same router the production emulator mounts.
//!
//! Two rules are under test, each asserted over a TABLE of inputs rather than the one
//! request a recording carried:
//!
//! * a quick response lives only in a knowledge base whose `knowledgeBaseType` holds quick
//!   responses, so EVERY operation of that plane refuses a knowledge base of any other
//!   modelled type with `ValidationException` (HTTP 400) naming that stored type, and
//!   serves it for a `QUICK_RESPONSES` one;
//! * a search reports the quick responses the knowledge base the route names HOLDS that
//!   the `searchExpression` selects — a record matching at least one `queries` entry (an
//!   expression carrying none selects the whole knowledge base) and every `filters` entry —
//!   projected out of the stored record, ordered by `orderOnField` and paged by
//!   `maxResults`/`nextToken`.
//!
//! The rows differ along what those rules generalise over: the knowledge-base type, the
//! operation of the plane, the queried field (each of the four the model documents as
//! queryable), the query operator (both modelled values), `allowFuzziness`, the filtered
//! field and its operator (both modelled values), `includeNoExistence`, how many values an
//! entry carries, how many records the expression selects (none, one, several, all), the
//! ordering and the page size.
//!
//! Every hit is compared against what `GetQuickResponse` answers for the same id, so a
//! result that is not computed from persisted state cannot pass. The harness is the one
//! every suite of this crate drives its own surface with (`content_search.rs`,
//! `quick_response.rs`): a router with just this service mounted on a fresh engine.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_wisdom_wisdomservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for.
const REGION: &str = "eu-central-1";

/// The endpoint host those requests address.
const ENDPOINT_HOST: &str = "wisdom.eu-central-1.amazonaws.com";

/// The routing key this service answers under (`src/service.rs`'s
/// `OWN_REGISTRATION_KEY`).
const ROUTE_KEY: &str = "com-amazonaws-wisdom-wisdomservice";

/// A UUID no create ever mints, for the not-found assertions.
const ABSENT_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// The modelled `KnowledgeBaseType` whose knowledge bases hold quick responses, and the
/// ones that hold none (`smithy_query(query="shape",shape="KnowledgeBaseType")` declares
/// exactly `CUSTOM`, `EXTERNAL` and `QUICK_RESPONSES`), so the table below covers the whole
/// catalog and not only the type the issue's recording used.
const TYPE_QUICK_RESPONSES: &str = "QUICK_RESPONSES";
const TYPES_WITHOUT_QUICK_RESPONSES: [&str; 2] = ["CUSTOM", "EXTERNAL"];

/// Both values the modelled `QuickResponseType` pattern admits
/// (`smithy_query(query="operation",operation="CreateQuickResponse")`: `contentType`
/// matches `^(application/x\.quickresponse;format=(plain|markdown))$`).
const CONTENT_TYPE_PLAIN: &str = "application/x.quickresponse;format=plain";
const CONTENT_TYPE_MARKDOWN: &str = "application/x.quickresponse;format=markdown";

/// The two modelled `QuickResponseQueryOperator` values
/// (`smithy_query(query="shape",shape="QuickResponseQueryOperator")`).
const CONTAINS: &str = "CONTAINS";
const CONTAINS_AND_PREFIX: &str = "CONTAINS_AND_PREFIX";

/// The two modelled `QuickResponseFilterOperator` values
/// (`smithy_query(query="shape",shape="QuickResponseFilterOperator")`).
const EQUALS: &str = "EQUALS";
const PREFIX: &str = "PREFIX";

/// The sentences this endpoint answers the two refusal classes below with, written out
/// here rather than imported from the implementation so the assertions pin the wire text.
const MESSAGE_KNOWLEDGE_BASE_NOT_FOUND: &str = "Knowledge Base does not exist.";
const MESSAGE_QUICK_RESPONSE_NOT_FOUND: &str = "QuickResponse does not exist.";

/// The sentence the quick-response plane refuses a knowledge base holding no quick
/// responses with: fixed text whose one variable part is the type of the knowledge base
/// the request named.
fn message_plane_unsupported(knowledge_base_type: &str) -> String {
    format!("QuickResponse APIs are not supported for {knowledge_base_type} knowledge bases")
}

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

    /// The list at `pointer`, or an empty one when the answer carries none.
    fn array(&self, pointer: &str) -> Vec<Value> {
        self.body
            .pointer(pointer)
            .and_then(Value::as_array)
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

    /// The `name`s of a search's `results`, in the order they were reported.
    fn hit_names(&self) -> Vec<String> {
        self.array("/results")
            .iter()
            .map(|hit| {
                hit.pointer("/name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// The `quickResponseId`s of a search's `results`, in the order they were reported.
    fn hit_ids(&self) -> Vec<String> {
        self.array("/results")
            .iter()
            .map(|hit| {
                hit.pointer("/quickResponseId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
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
        // SigV4 credential scope below, which is also where an ARN's region comes from.
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

    /// A knowledge base of type `kind`, answering with the id its create persisted.
    async fn knowledge_base(&self, name: &str, kind: &str) -> String {
        let reply = self
            .send(
                "POST",
                "/knowledgeBases",
                Some(json!({ "name": name, "knowledgeBaseType": kind })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"knowledge base {name} of type {kind}\": expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        reply.string("/knowledgeBase/knowledgeBaseId")
    }

    async fn create_quick_response(&self, knowledge_base: &str, body: Value) -> Reply {
        self.send(
            "POST",
            &format!("/knowledgeBases/{knowledge_base}/quickResponses"),
            Some(body),
        )
        .await
    }

    async fn get_quick_response(&self, knowledge_base: &str, quick_response: &str) -> Reply {
        self.send(
            "GET",
            &format!("/knowledgeBases/{knowledge_base}/quickResponses/{quick_response}"),
            None,
        )
        .await
    }

    async fn delete_quick_response(&self, knowledge_base: &str, quick_response: &str) -> Reply {
        self.send(
            "DELETE",
            &format!("/knowledgeBases/{knowledge_base}/quickResponses/{quick_response}"),
            None,
        )
        .await
    }

    async fn list_quick_responses(&self, knowledge_base: &str) -> Reply {
        self.send(
            "GET",
            &format!("/knowledgeBases/{knowledge_base}/quickResponses?maxResults=100"),
            None,
        )
        .await
    }

    /// `SearchQuickResponses` with one expression and whatever query string the paging row
    /// asks for.
    async fn search(&self, knowledge_base: &str, expression: Value, query: &str) -> Reply {
        self.send(
            "POST",
            &format!("/knowledgeBases/{knowledge_base}/search/quickResponses{query}"),
            Some(json!({ "searchExpression": expression })),
        )
        .await
    }

    /// The whole knowledge base, through the expression that selects it: no query at all.
    async fn search_all(&self, knowledge_base: &str) -> Reply {
        self.search(knowledge_base, json!({}), "").await
    }
}

/// One `queries` entry.
fn query(name: &str, values: &[&str], operator: &str, allow_fuzziness: Option<bool>) -> Value {
    let mut entry = json!({ "name": name, "values": values, "operator": operator });
    if let Some(allow_fuzziness) = allow_fuzziness {
        entry["allowFuzziness"] = json!(allow_fuzziness);
    }
    entry
}

/// One `filters` entry.
fn filter(
    name: &str,
    values: Option<&[&str]>,
    operator: &str,
    include_no_existence: Option<bool>,
) -> Value {
    let mut entry = json!({ "name": name, "operator": operator });
    if let Some(values) = values {
        entry["values"] = json!(values);
    }
    if let Some(include_no_existence) = include_no_existence {
        entry["includeNoExistence"] = json!(include_no_existence);
    }
    entry
}

/// One quick response the suite seeds its knowledge base with. The fields are the ones a
/// search can query, filter and order on, so one table of records serves every row below.
struct Seed {
    name: &'static str,
    content: &'static str,
    content_type: &'static str,
    description: Option<&'static str>,
    shortcut_key: Option<&'static str>,
    /// `None` exercises a create that names no `isActive` at all.
    is_active: Option<bool>,
    channels: &'static [&'static str],
    language: Option<&'static str>,
    grouping_criteria: Option<&'static str>,
    grouping_values: &'static [&'static str],
}

impl Seed {
    fn create_body(&self) -> Value {
        let mut body = json!({
            "name": self.name,
            "content": { "content": self.content },
            "contentType": self.content_type,
        });
        if let Some(description) = self.description {
            body["description"] = json!(description);
        }
        if let Some(shortcut_key) = self.shortcut_key {
            body["shortcutKey"] = json!(shortcut_key);
        }
        if let Some(is_active) = self.is_active {
            body["isActive"] = json!(is_active);
        }
        if !self.channels.is_empty() {
            body["channels"] = json!(self.channels);
        }
        if let Some(language) = self.language {
            body["language"] = json!(language);
        }
        if self.grouping_criteria.is_some() || !self.grouping_values.is_empty() {
            let mut grouping = json!({});
            if let Some(criteria) = self.grouping_criteria {
                grouping["criteria"] = json!(criteria);
            }
            if !self.grouping_values.is_empty() {
                grouping["values"] = json!(self.grouping_values);
            }
            body["groupingConfiguration"] = grouping;
        }
        body
    }
}

/// The seeded knowledge base: records differing in every searchable field, so a row can
/// select one, several, all or none of them. Two names share a prefix and one is a case
/// variant of another, which is what the prefix and the fuzziness rows turn on. The
/// language codes come from the catalog the model documents for `language` (de_DE, en_US,
/// es_ES, fr_FR, id_ID, it_IT, ja_JP, ko_KR, pt_BR, zh_CN, zh_TW).
const SEEDS: [Seed; 5] = [
    Seed {
        name: "order-shipped",
        content: "Your order has shipped!",
        content_type: CONTENT_TYPE_PLAIN,
        description: Some("shipping update"),
        shortcut_key: Some("/ship"),
        is_active: None,
        channels: &["Chat"],
        language: Some("en_US"),
        grouping_criteria: Some("RoutingProfileArn"),
        grouping_values: &["sales"],
    },
    Seed {
        name: "order-cancelled",
        content: "Your order was cancelled.",
        content_type: CONTENT_TYPE_PLAIN,
        description: Some("cancellation notice"),
        shortcut_key: Some("/cancel"),
        is_active: Some(false),
        channels: &["Chat", "Email"],
        language: Some("en_US"),
        grouping_criteria: None,
        grouping_values: &[],
    },
    Seed {
        name: "GREETING-formal",
        content: "Good day, how may I help?",
        content_type: CONTENT_TYPE_MARKDOWN,
        description: Some("a formal greeting"),
        shortcut_key: Some("/hi"),
        is_active: Some(true),
        channels: &[],
        language: Some("de_DE"),
        grouping_criteria: Some("RoutingProfileArn"),
        grouping_values: &["service", "sales"],
    },
    Seed {
        name: "greeting-casual",
        content: "Hey! How can I help today?",
        content_type: CONTENT_TYPE_MARKDOWN,
        description: None,
        shortcut_key: None,
        is_active: Some(true),
        channels: &["Email"],
        language: Some("zh_TW"),
        grouping_criteria: None,
        grouping_values: &[],
    },
    Seed {
        name: "refund-policy",
        content: "Refunds are issued within 14 days.",
        content_type: CONTENT_TYPE_PLAIN,
        description: Some("the refund rule"),
        shortcut_key: Some("/refund"),
        is_active: Some(true),
        channels: &["Chat", "Email"],
        language: Some("fr_FR"),
        grouping_criteria: Some("Department"),
        grouping_values: &["finance"],
    },
];

/// Create every seed under `knowledge_base_id`, answering with the `(name, id)` pairs the
/// creates persisted.
async fn seed(emulator: &Emulator, knowledge_base_id: &str) -> Vec<(&'static str, String)> {
    let mut created = Vec::new();
    for row in &SEEDS {
        let reply = emulator
            .create_quick_response(knowledge_base_id, row.create_body())
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"seed {}\": expected HTTP 200 from the create, got {} with {}",
            row.name, reply.status, reply.body
        );
        created.push((row.name, reply.string("/quickResponse/quickResponseId")));
    }
    created
}

/// RULE: a quick response lives only in a knowledge base whose type holds quick responses,
/// so every operation of that plane refuses a knowledge base of any other modelled type
/// with HTTP 400 `ValidationException` naming that stored type — and serves the same calls
/// against a `QUICK_RESPONSES` knowledge base.
#[tokio::test]
async fn the_quick_response_plane_is_refused_for_a_knowledge_base_type_that_holds_none() {
    let emulator = Emulator::new();

    for knowledge_base_type in TYPES_WITHOUT_QUICK_RESPONSES {
        let knowledge_base_id = emulator
            .knowledge_base(
                &format!("gated-kb-{}", knowledge_base_type.to_lowercase()),
                knowledge_base_type,
            )
            .await;
        let expected = message_plane_unsupported(knowledge_base_type);

        // Every operation of the plane, each with a request that would otherwise be
        // served: the create carries a well-formed body, and the three label-addressed
        // ones name a legal `UuidOrArn` so the refusal is the gate's and not the
        // boundary's.
        let rows: Vec<(String, &str, String, Option<Value>)> = vec![
            (
                format!("CreateQuickResponse against a {knowledge_base_type} knowledge base"),
                "POST",
                format!("/knowledgeBases/{knowledge_base_id}/quickResponses"),
                Some(json!({ "name": "gated", "content": { "content": "gated" } })),
            ),
            (
                format!("ListQuickResponses against a {knowledge_base_type} knowledge base"),
                "GET",
                format!("/knowledgeBases/{knowledge_base_id}/quickResponses"),
                None,
            ),
            (
                format!("GetQuickResponse against a {knowledge_base_type} knowledge base"),
                "GET",
                format!("/knowledgeBases/{knowledge_base_id}/quickResponses/{ABSENT_UUID}"),
                None,
            ),
            (
                format!("UpdateQuickResponse against a {knowledge_base_type} knowledge base"),
                "POST",
                format!("/knowledgeBases/{knowledge_base_id}/quickResponses/{ABSENT_UUID}"),
                Some(json!({ "name": "gated" })),
            ),
            (
                format!("DeleteQuickResponse against a {knowledge_base_type} knowledge base"),
                "DELETE",
                format!("/knowledgeBases/{knowledge_base_id}/quickResponses/{ABSENT_UUID}"),
                None,
            ),
            (
                format!("SearchQuickResponses against a {knowledge_base_type} knowledge base"),
                "POST",
                format!("/knowledgeBases/{knowledge_base_id}/search/quickResponses"),
                Some(json!({
                    "searchExpression": {
                        "queries": [ { "name": "name", "values": ["gated"], "operator": CONTAINS } ]
                    }
                })),
            ),
        ];

        for (case, method, uri, body) in rows {
            let refused = emulator.send(method, &uri, body).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: expected HTTP 400, got {} with {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?} with {}",
                refused.code(),
                refused.body
            );
            assert_eq!(
                refused.message(),
                expected,
                "case {case:?}: expected the refusal to name the stored knowledge-base type, got {}",
                refused.body
            );
        }
    }

    // The positive control: the same six calls against a QUICK_RESPONSES knowledge base
    // are served, so the gate refuses the type and not the plane.
    let served_kb = emulator
        .knowledge_base("ungated-kb", TYPE_QUICK_RESPONSES)
        .await;
    let created = emulator
        .create_quick_response(
            &served_kb,
            json!({ "name": "ungated", "content": { "content": "ungated" } }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"CreateQuickResponse against a QUICK_RESPONSES knowledge base\": expected \
         HTTP 200, got {} with {}",
        created.status, created.body
    );
    let quick_response_id = created.string("/quickResponse/quickResponseId");
    for (case, reply, expected_status) in [
        (
            "ListQuickResponses against a QUICK_RESPONSES knowledge base",
            emulator.list_quick_responses(&served_kb).await,
            200,
        ),
        (
            "GetQuickResponse against a QUICK_RESPONSES knowledge base",
            emulator
                .get_quick_response(&served_kb, &quick_response_id)
                .await,
            200,
        ),
        (
            "SearchQuickResponses against a QUICK_RESPONSES knowledge base",
            emulator.search_all(&served_kb).await,
            200,
        ),
        (
            "UpdateQuickResponse against a QUICK_RESPONSES knowledge base",
            emulator
                .send(
                    "POST",
                    &format!("/knowledgeBases/{served_kb}/quickResponses/{quick_response_id}"),
                    Some(json!({ "description": "still ungated" })),
                )
                .await,
            200,
        ),
        (
            "DeleteQuickResponse against a QUICK_RESPONSES knowledge base",
            emulator
                .delete_quick_response(&served_kb, &quick_response_id)
                .await,
            204,
        ),
    ] {
        assert_eq!(
            reply.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} with {}",
            reply.status, reply.body
        );
    }
}

/// RULE: a search reports the quick responses the knowledge base HOLDS that the expression
/// selects, under the operator the entry names — over every queryable field, both query
/// operators, and `allowFuzziness` absent, false and true.
#[tokio::test]
async fn a_query_selects_the_stored_quick_responses_its_field_matches() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base("search-kb", TYPE_QUICK_RESPONSES)
        .await;
    let created = seed(&emulator, &knowledge_base_id).await;

    // Each row: the case, the expression, and the names the search must report — which is
    // the whole of the rule, stated per row rather than recomputed from the seeds.
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "CONTAINS on name, the field the issue's checks query",
            json!({ "queries": [query("name", &["order-shipped"], CONTAINS, None)] }),
            vec!["order-shipped"],
        ),
        (
            "CONTAINS on name matching a substring of two records",
            json!({ "queries": [query("name", &["order-"], CONTAINS, None)] }),
            vec!["order-cancelled", "order-shipped"],
        ),
        (
            "CONTAINS on content, a field no name carries the value of",
            json!({ "queries": [query("content", &["within 14 days"], CONTAINS, None)] }),
            vec!["refund-policy"],
        ),
        (
            "CONTAINS on description",
            json!({ "queries": [query("description", &["cancellation"], CONTAINS, None)] }),
            vec!["order-cancelled"],
        ),
        (
            "CONTAINS on shortcutKey",
            json!({ "queries": [query("shortcutKey", &["/refund"], CONTAINS, None)] }),
            vec!["refund-policy"],
        ),
        (
            "CONTAINS with several values, which are alternatives",
            json!({
                "queries": [query("name", &["refund", "casual"], CONTAINS, None)]
            }),
            vec!["greeting-casual", "refund-policy"],
        ),
        (
            "CONTAINS with two queries, which are alternatives too",
            json!({
                "queries": [
                    query("name", &["refund"], CONTAINS, None),
                    query("content", &["Good day"], CONTAINS, None),
                ]
            }),
            vec!["GREETING-formal", "refund-policy"],
        ),
        (
            "CONTAINS selecting nothing the knowledge base holds",
            json!({ "queries": [query("name", &["no-such-quick-response"], CONTAINS, None)] }),
            Vec::new(),
        ),
        (
            "CONTAINS in the wrong case without fuzziness is an exact match",
            json!({ "queries": [query("name", &["greeting-FORMAL"], CONTAINS, Some(false))] }),
            Vec::new(),
        ),
        (
            "CONTAINS in the wrong case with fuzziness matches",
            json!({ "queries": [query("name", &["greeting-FORMAL"], CONTAINS, Some(true))] }),
            vec!["GREETING-formal"],
        ),
        (
            "CONTAINS_AND_PREFIX on a word prefix of the content",
            json!({ "queries": [query("content", &["Refunds are iss"], CONTAINS_AND_PREFIX, None)] }),
            vec!["refund-policy"],
        ),
        (
            "an expression carrying no query selects the whole knowledge base",
            json!({}),
            vec![
                "GREETING-formal",
                "greeting-casual",
                "order-cancelled",
                "order-shipped",
                "refund-policy",
            ],
        ),
    ];

    for (case, expression, expected) in rows {
        let found = emulator
            .search(&knowledge_base_id, expression, "")
            .await;
        assert_eq!(
            found.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {}",
            found.status, found.body
        );
        let mut names = found.hit_names();
        names.sort();
        let mut expected: Vec<String> = expected.iter().map(|name| name.to_string()).collect();
        expected.sort();
        assert_eq!(
            names, expected,
            "case {case:?}: expected the search to report {expected:?}, got {names:?} in {}",
            found.body
        );
    }

    // Every hit is the STORED record: each member a search result declares equals what a
    // read of the same quick response answers with.
    let found = emulator.search_all(&knowledge_base_id).await;
    for (name, quick_response_id) in &created {
        let hit = found
            .array("/results")
            .into_iter()
            .find(|hit| hit.pointer("/quickResponseId").and_then(Value::as_str) == Some(quick_response_id))
            .unwrap_or_else(|| panic!("case {name:?}: the search reported no hit for {quick_response_id}"));
        let read = emulator
            .get_quick_response(&knowledge_base_id, quick_response_id)
            .await;
        assert_eq!(
            read.status, 200,
            "case {name:?}: expected HTTP 200 from the read, got {} with {}",
            read.status, read.body
        );
        let data = read.object("/quickResponse");
        for member in [
            "quickResponseArn",
            "quickResponseId",
            "knowledgeBaseArn",
            "knowledgeBaseId",
            "name",
            "contentType",
            "status",
            "contents",
            "createdTime",
            "lastModifiedTime",
            "lastModifiedBy",
            "description",
            "shortcutKey",
            "groupingConfiguration",
            "channels",
            "language",
        ] {
            assert_eq!(
                hit.get(member),
                data.get(member),
                "case {name:?}, member {member:?}: a search hit and a read of the same record \
                 must agree, got {:?} against {:?}",
                hit.get(member),
                data.get(member)
            );
        }
        // `isActive` is REQUIRED on a search hit and optional on the read, so a record
        // whose create named none reports the value it is in service with rather than
        // nothing at all.
        let expected_active = data.get("isActive").and_then(Value::as_bool).unwrap_or(true);
        assert_eq!(
            hit.pointer("/isActive").and_then(Value::as_bool),
            Some(expected_active),
            "case {name:?}: expected isActive {expected_active}, got {hit}"
        );
    }
}

/// RULE: a `CONTAINS_AND_PREFIX` query returns exactly the quick responses whose field
/// carries the shared prefix and excludes the unrelated ones — the search-as-you-type
/// reading of the operator, asserted on every prefix length of a shared name and on a
/// prefix no record carries.
#[tokio::test]
async fn a_contains_and_prefix_query_selects_exactly_the_records_sharing_the_prefix() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base("prefix-kb", TYPE_QUICK_RESPONSES)
        .await;
    seed(&emulator, &knowledge_base_id).await;

    // Each row: the prefix queried, and the names it must select. `order` and its longer
    // prefixes narrow from two records to one; `greeting` selects only the lower-case
    // record without fuzziness and both with it; a prefix no name carries selects none.
    let rows: Vec<(&str, &str, Option<bool>, Vec<&str>)> = vec![
        (
            "a prefix two names share",
            "order",
            None,
            vec!["order-cancelled", "order-shipped"],
        ),
        (
            "a longer prefix of the same two names",
            "order-c",
            None,
            vec!["order-cancelled"],
        ),
        (
            "the whole name as its own prefix",
            "order-shipped",
            None,
            vec!["order-shipped"],
        ),
        (
            "a prefix no name carries",
            "zzz",
            None,
            Vec::new(),
        ),
        (
            "a prefix in the wrong case, exact matching",
            "GREETING",
            Some(false),
            vec!["GREETING-formal"],
        ),
        (
            "the same prefix with fuzziness, which matches both",
            "GREETING",
            Some(true),
            vec!["GREETING-formal", "greeting-casual"],
        ),
    ];

    for (case, prefix, allow_fuzziness, expected) in rows {
        let found = emulator
            .search(
                &knowledge_base_id,
                json!({ "queries": [query("name", &[prefix], CONTAINS_AND_PREFIX, allow_fuzziness)] }),
                "",
            )
            .await;
        assert_eq!(
            found.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {}",
            found.status, found.body
        );
        let mut names = found.hit_names();
        names.sort();
        let mut expected: Vec<String> = expected.iter().map(|name| name.to_string()).collect();
        expected.sort();
        assert_eq!(
            names, expected,
            "case {case:?}: expected the prefix query to report {expected:?}, got {names:?} in {}",
            found.body
        );
    }
}

/// RULE: a search reports what the knowledge base HOLDS, so a deleted quick response stops
/// being returned — and the read of it is then the modelled not-found.
#[tokio::test]
async fn a_deleted_quick_response_stops_being_a_search_hit() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base("delete-kb", TYPE_QUICK_RESPONSES)
        .await;
    let created = seed(&emulator, &knowledge_base_id).await;

    // Delete them one at a time, so the rule is asserted at every size of the collection
    // from all of them down to none.
    let mut surviving: Vec<(&str, String)> = created.clone();
    while let Some((name, quick_response_id)) = surviving.pop() {
        let deleted = emulator
            .delete_quick_response(&knowledge_base_id, &quick_response_id)
            .await;
        assert_eq!(
            deleted.status, 204,
            "case \"delete {name}\": expected HTTP 204, got {} with {}",
            deleted.status, deleted.body
        );

        let found = emulator.search_all(&knowledge_base_id).await;
        assert!(
            !found.hit_ids().contains(&quick_response_id),
            "case \"delete {name}\": the deleted id must stop being a search hit, got {}",
            found.body
        );
        let mut names = found.hit_names();
        names.sort();
        let mut expected: Vec<String> = surviving
            .iter()
            .map(|(name, _)| name.to_string())
            .collect();
        expected.sort();
        assert_eq!(
            names, expected,
            "case \"delete {name}\": expected the survivors {expected:?}, got {names:?} in {}",
            found.body
        );

        // A query naming the deleted record's own name selects nothing either, so the
        // disappearance is of the record and not of the page it sat on.
        let by_name = emulator
            .search(
                &knowledge_base_id,
                json!({ "queries": [query("name", &[name], CONTAINS, None)] }),
                "",
            )
            .await;
        assert!(
            !by_name.hit_ids().contains(&quick_response_id),
            "case \"delete {name}\": a query on the deleted name must not find it, got {}",
            by_name.body
        );

        let read = emulator
            .get_quick_response(&knowledge_base_id, &quick_response_id)
            .await;
        assert_eq!(
            read.status, 404,
            "case \"delete {name}\": expected HTTP 404 from the read, got {} with {}",
            read.status, read.body
        );
        assert_eq!(
            read.code(),
            "ResourceNotFoundException",
            "case \"delete {name}\": expected ResourceNotFoundException, got {:?} with {}",
            read.code(),
            read.body
        );
        assert_eq!(
            read.message(),
            MESSAGE_QUICK_RESPONSE_NOT_FOUND,
            "case \"delete {name}\": unexpected message in {}",
            read.body
        );
    }
}

/// RULE: a search expression admits only what EVERY filter accepts, under that filter's
/// operator, and `includeNoExistence` is what decides a record carrying no value for the
/// filtered field — over every field the model documents as filterable.
#[tokio::test]
async fn every_filter_must_admit_a_record_for_it_to_be_reported() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base("filter-kb", TYPE_QUICK_RESPONSES)
        .await;
    seed(&emulator, &knowledge_base_id).await;

    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "EQUALS on name",
            json!({ "filters": [filter("name", Some(&["refund-policy"]), EQUALS, None)] }),
            vec!["refund-policy"],
        ),
        (
            "EQUALS on name in the wrong case matches nothing",
            json!({ "filters": [filter("name", Some(&["REFUND-POLICY"]), EQUALS, None)] }),
            Vec::new(),
        ),
        (
            "PREFIX on name",
            json!({ "filters": [filter("name", Some(&["greeting"]), PREFIX, None)] }),
            vec!["greeting-casual"],
        ),
        (
            "EQUALS on isActive, which a record naming none reports by default",
            json!({ "filters": [filter("isActive", Some(&["true"]), EQUALS, None)] }),
            vec![
                "GREETING-formal",
                "greeting-casual",
                "order-shipped",
                "refund-policy",
            ],
        ),
        (
            "EQUALS on isActive false",
            json!({ "filters": [filter("isActive", Some(&["false"]), EQUALS, None)] }),
            vec!["order-cancelled"],
        ),
        (
            "EQUALS on channels, a list-valued field",
            json!({ "filters": [filter("channels", Some(&["Email"]), EQUALS, None)] }),
            vec!["greeting-casual", "order-cancelled", "refund-policy"],
        ),
        (
            "EQUALS on language",
            json!({ "filters": [filter("language", Some(&["en_US"]), EQUALS, None)] }),
            vec!["order-cancelled", "order-shipped"],
        ),
        (
            "EQUALS on contentType",
            json!({ "filters": [filter("contentType", Some(&[CONTENT_TYPE_MARKDOWN]), EQUALS, None)] }),
            vec!["GREETING-formal", "greeting-casual"],
        ),
        (
            "EQUALS on groupingConfiguration.criteria",
            json!({
                "filters": [filter(
                    "groupingConfiguration.criteria",
                    Some(&["RoutingProfileArn"]),
                    EQUALS,
                    None,
                )]
            }),
            vec!["GREETING-formal", "order-shipped"],
        ),
        (
            "EQUALS on groupingConfiguration.values, a list-valued field",
            json!({
                "filters": [filter(
                    "groupingConfiguration.values",
                    Some(&["sales"]),
                    EQUALS,
                    None,
                )]
            }),
            vec!["GREETING-formal", "order-shipped"],
        ),
        (
            "a filter on a field a record carries no value for excludes it",
            json!({ "filters": [filter("shortcutKey", Some(&["/hi"]), EQUALS, None)] }),
            vec!["GREETING-formal"],
        ),
        (
            "includeNoExistence admits exactly the records carrying no value",
            json!({
                "filters": [filter("description", None, EQUALS, Some(true))]
            }),
            vec!["greeting-casual"],
        ),
        (
            "two filters are a conjunction",
            json!({
                "filters": [
                    filter("channels", Some(&["Chat"]), EQUALS, None),
                    filter("language", Some(&["en_US"]), EQUALS, None),
                ]
            }),
            vec!["order-cancelled", "order-shipped"],
        ),
        (
            "two filters that cannot both hold select nothing",
            json!({
                "filters": [
                    filter("language", Some(&["en_US"]), EQUALS, None),
                    filter("language", Some(&["fr_FR"]), EQUALS, None),
                ]
            }),
            Vec::new(),
        ),
        (
            "a query and a filter must BOTH hold",
            json!({
                "queries": [query("name", &["order-"], CONTAINS, None)],
                "filters": [filter("isActive", Some(&["false"]), EQUALS, None)],
            }),
            vec!["order-cancelled"],
        ),
    ];

    for (case, expression, expected) in rows {
        let found = emulator
            .search(&knowledge_base_id, expression, "")
            .await;
        assert_eq!(
            found.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {}",
            found.status, found.body
        );
        let mut names = found.hit_names();
        names.sort();
        let mut expected: Vec<String> = expected.iter().map(|name| name.to_string()).collect();
        expected.sort();
        assert_eq!(
            names, expected,
            "case {case:?}: expected the filtered search to report {expected:?}, got {names:?} \
             in {}",
            found.body
        );
    }

    // A field outside the model's own catalog is reported rather than answered with a page
    // that selects nothing, which a client could not tell from "the knowledge base holds
    // no match".
    for (case, expression) in [
        (
            "a queries entry naming a field outside the queryable catalog",
            json!({ "queries": [query("isActive", &["true"], CONTAINS, None)] }),
        ),
        (
            "a filters entry naming a field outside the filterable catalog",
            json!({ "filters": [filter("content", Some(&["x"]), EQUALS, None)] }),
        ),
        (
            "an orderOnField naming a field outside the filterable catalog",
            json!({ "orderOnField": { "name": "content" } }),
        ),
    ] {
        let refused = emulator.search(&knowledge_base_id, expression, "").await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?} with {}",
            refused.code(),
            refused.body
        );
    }
}

/// RULE: the hits are served in the order `orderOnField` names and walked by
/// `maxResults`/`nextToken`, whose token continues THAT order — over every page size from
/// one to past the collection, both directions, and with no ordering named at all.
#[tokio::test]
async fn the_hits_are_ordered_and_paged_and_a_token_continues_its_own_order() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base("paging-kb", TYPE_QUICK_RESPONSES)
        .await;
    seed(&emulator, &knowledge_base_id).await;

    // Ordered on `name`, both directions: the whole collection in one page states the
    // order the paged walks below must reproduce.
    let ascending = emulator
        .search(
            &knowledge_base_id,
            json!({ "orderOnField": { "name": "name", "order": "ASC" } }),
            "",
        )
        .await;
    let mut expected_ascending: Vec<String> = SEEDS.iter().map(|seed| seed.name.to_string()).collect();
    expected_ascending.sort();
    assert_eq!(
        ascending.hit_names(),
        expected_ascending,
        "case \"ordered on name ASC\": expected {expected_ascending:?}, got {:?} in {}",
        ascending.hit_names(),
        ascending.body
    );
    let descending = emulator
        .search(
            &knowledge_base_id,
            json!({ "orderOnField": { "name": "name", "order": "DESC" } }),
            "",
        )
        .await;
    let expected_descending: Vec<String> =
        expected_ascending.iter().rev().cloned().collect();
    assert_eq!(
        descending.hit_names(),
        expected_descending,
        "case \"ordered on name DESC\": expected {expected_descending:?}, got {:?} in {}",
        descending.hit_names(),
        descending.body
    );

    // Walked to exhaustion at every page size from one to one past the collection, under
    // each ordering: the pages concatenate to exactly the one-page answer.
    for (case, expression, expected) in [
        (
            "no ordering named",
            json!({}),
            emulator.search_all(&knowledge_base_id).await.hit_names(),
        ),
        (
            "ordered on name ASC",
            json!({ "orderOnField": { "name": "name", "order": "ASC" } }),
            expected_ascending.clone(),
        ),
        (
            "ordered on name DESC",
            json!({ "orderOnField": { "name": "name", "order": "DESC" } }),
            expected_descending.clone(),
        ),
        (
            "ordered on lastModifiedTime, a timestamp field",
            json!({ "orderOnField": { "name": "lastModifiedTime", "order": "ASC" } }),
            emulator
                .search(
                    &knowledge_base_id,
                    json!({ "orderOnField": { "name": "lastModifiedTime", "order": "ASC" } }),
                    "",
                )
                .await
                .hit_names(),
        ),
    ] {
        for page_size in 1..=SEEDS.len() + 1 {
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let query_string = match &token {
                    None => format!("?maxResults={page_size}"),
                    Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                };
                let page = emulator
                    .search(&knowledge_base_id, expression.clone(), &query_string)
                    .await;
                assert_eq!(
                    page.status, 200,
                    "case {case:?} at page size {page_size}: expected HTTP 200, got {} with {}",
                    page.status, page.body
                );
                let names = page.hit_names();
                assert!(
                    names.len() <= page_size,
                    "case {case:?} at page size {page_size}: a page must hold at most \
                     {page_size} hits, got {names:?}"
                );
                walked.extend(names);
                pages += 1;
                assert!(
                    pages <= SEEDS.len() + 2,
                    "case {case:?} at page size {page_size}: the walk must terminate, got \
                     {walked:?}"
                );
                match page.body.pointer("/nextToken").and_then(Value::as_str) {
                    Some(next) => token = Some(next.to_string()),
                    None => break,
                }
            }
            assert_eq!(
                walked, expected,
                "case {case:?} at page size {page_size}: the pages must concatenate to the \
                 one-page answer {expected:?}, got {walked:?}"
            );
        }
    }

    // A token is a position in ONE order, so one minted under an ordering and replayed
    // against another names no position in that search and is refused — as is a token this
    // service never minted.
    let first = emulator
        .search(
            &knowledge_base_id,
            json!({ "orderOnField": { "name": "name", "order": "ASC" } }),
            "?maxResults=1",
        )
        .await;
    let token = first
        .body
        .pointer("/nextToken")
        .and_then(Value::as_str)
        .expect("the first page of five records carries a continuation token")
        .to_string();
    for (case, expression, token) in [
        (
            "a token minted under ASC replayed under DESC",
            json!({ "orderOnField": { "name": "name", "order": "DESC" } }),
            token.clone(),
        ),
        (
            "a token minted under an ordering replayed with none",
            json!({}),
            token.clone(),
        ),
        (
            "a token this service never minted",
            json!({}),
            "not-a-token-of-this-search".to_string(),
        ),
    ] {
        let refused = emulator
            .search(
                &knowledge_base_id,
                expression,
                &format!("?maxResults=1&nextToken={token}"),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?} with {}",
            refused.code(),
            refused.body
        );
    }
}

/// RULE: a search REPORTS; it writes nothing, so repeating one answers alike and the
/// records it reported are untouched by it. A knowledge base this account does not hold is
/// the modelled not-found rather than an empty page.
#[tokio::test]
async fn a_search_reports_without_writing_and_is_not_found_for_an_unknown_knowledge_base() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base("read-only-kb", TYPE_QUICK_RESPONSES)
        .await;
    let created = seed(&emulator, &knowledge_base_id).await;

    let before = emulator.search_all(&knowledge_base_id).await;
    let listed_before = emulator.list_quick_responses(&knowledge_base_id).await;
    for round in 1..=3 {
        let again = emulator.search_all(&knowledge_base_id).await;
        assert_eq!(
            again.body.pointer("/results"),
            before.body.pointer("/results"),
            "case \"repeat {round}\": repeated searches must answer alike, got {}",
            again.body
        );
    }
    let listed_after = emulator.list_quick_responses(&knowledge_base_id).await;
    assert_eq!(
        listed_after.body.pointer("/quickResponseSummaries"),
        listed_before.body.pointer("/quickResponseSummaries"),
        "case \"the listing after the searches\": a search must leave the records untouched, \
         got {}",
        listed_after.body
    );
    for (name, quick_response_id) in &created {
        let read = emulator
            .get_quick_response(&knowledge_base_id, quick_response_id)
            .await;
        assert_eq!(
            read.status, 200,
            "case {name:?}: the record must still be readable after the searches, got {} \
             with {}",
            read.status, read.body
        );
    }

    // A knowledge base this account holds none under — a bare id nothing minted, and an
    // ARN of this account naming no knowledge base.
    for (case, label) in [
        ("a bare id no create minted", ABSENT_UUID.to_string()),
        (
            "an ARN of this account naming no knowledge base",
            format!(
                "arn:aws:wisdom:{REGION}:{account}:knowledge-base/{ABSENT_UUID}",
                account = engine::EMULATOR_ACCOUNT_ID
            )
            .replace('/', "%2F"),
        ),
    ] {
        let refused = emulator.search_all(&label).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404, got {} with {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?} with {}",
            refused.code(),
            refused.body
        );
        assert_eq!(
            refused.message(),
            MESSAGE_KNOWLEDGE_BASE_NOT_FOUND,
            "case {case:?}: unexpected message in {}",
            refused.body
        );
    }

    // A quick response of ANOTHER knowledge base is never a hit: the collection a search
    // selects out of is the one the route names.
    let other_kb = emulator
        .knowledge_base("read-only-other-kb", TYPE_QUICK_RESPONSES)
        .await;
    let elsewhere = emulator.search_all(&other_kb).await;
    assert_eq!(
        elsewhere.hit_names(),
        Vec::<String>::new(),
        "case \"another knowledge base\": expected no hit, got {}",
        elsewhere.body
    );
}
