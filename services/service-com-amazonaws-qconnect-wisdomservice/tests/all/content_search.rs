// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `SearchContent`, driven over HTTP through the same router the
//! production emulator mounts.
//!
//! The rules under test, each asserted over a TABLE of contents and expressions rather
//! than one recorded request:
//!
//! * a search REPORTS the contents of the knowledge base the route names that satisfy
//!   every filter of the search expression — the summaries being the stored records, equal
//!   member for member to the ones the plain listing of the same collection reports;
//! * a filter selects under its operator: `EQUALS` on `NAME` accepts the content whose
//!   name is that value and nothing that merely resembles it (another case, a prefix, a
//!   suffix, surrounding space);
//! * a search expression is the CONJUNCTION of its filters: no filter selects the whole
//!   collection, two filters agreeing on a name select that one content, two disagreeing
//!   select none;
//! * the matches are ordered by `contentId` and `maxResults`/`nextToken` walk them, over
//!   every page size the model allows, including both published bounds and one past each,
//!   while a token this service never minted is refused;
//! * a knowledge base label this account holds none under is the modelled
//!   `ResourceNotFoundException` (HTTP 404), and a content of ANOTHER knowledge base is
//!   never a match;
//! * the search writes nothing: repeated searches answer alike and the asset locator of a
//!   matched content is untouched by them.
//!
//! Everything goes over the wire, so a response that is not computed from persisted state
//! cannot pass.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_qconnect_wisdomservice::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

// The percent-encoding an AWS SDK applies to a non-greedy `@httpLabel`, as this crate's
// test tree already exports it.
use super::resource_tags::label as encode_label;

/// The region every request in this suite is signed for.
const REGION: &str = "us-west-2";

/// The endpoint host those requests address.
const ENDPOINT_HOST: &str = "wisdom.us-west-2.amazonaws.com";

/// A UUID no create ever mints, for the not-found assertions. The all-zero id is NOT
/// one: it names the system-managed session every assistant is provisioned with.
const ABSENT_UUID: &str = "ffffffff-ffff-ffff-ffff-ffffffffffff";

/// The one `FilterField` the model declares
/// (`smithy_query(query="shape",shape="FilterField")`).
const FIELD_NAME: &str = "NAME";

/// The one `FilterOperator` the model declares
/// (`smithy_query(query="shape",shape="FilterOperator")`).
const OPERATOR_EQUALS: &str = "EQUALS";

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

    /// The `contentId`s of a `contentSummaries` list, in the order they were reported.
    fn matched_ids(&self) -> Vec<String> {
        self.array("/contentSummaries")
            .iter()
            .map(|summary| {
                summary
                    .pointer("/contentId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
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

/// A search expression selecting on `NAME` with `EQUALS`, one filter per name.
fn by_names(names: &[&str]) -> Value {
    json!({
        "filters": names
            .iter()
            .map(|name| json!({
                "field": FIELD_NAME,
                "operator": OPERATOR_EQUALS,
                "value": name,
            }))
            .collect::<Vec<Value>>(),
    })
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

    /// `SearchContent` under one knowledge base label, with the given expression and
    /// query string.
    async fn search(&self, knowledge_base_label: &str, expression: Value, query: &str) -> Reply {
        self.send(
            "POST",
            &format!(
                "/knowledgeBases/{}/search{query}",
                encode_label(knowledge_base_label)
            ),
            Some(json!({ "searchExpression": expression })),
        )
        .await
    }

    /// `ListContents` for one knowledge base — the plain listing of the collection a
    /// search selects out of.
    async fn list(&self, knowledge_base_id: &str) -> Reply {
        self.send(
            "GET",
            &format!("/knowledgeBases/{knowledge_base_id}/contents?maxResults=100"),
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

/// The contents every search in this suite selects out of. The media types are drawn from
/// the whole modelled `ContentType` catalog
/// (`smithy_query(query="shape",shape="ContentType")`:
/// `^(text/(plain|html|csv))|(application/(pdf|vnd\.openxmlformats-officedocument\.wordprocessingml\.document))|(application/x\.wisdom-json;source=(salesforce|servicenow|zendesk))$`),
/// including entries the issue cited none of, and the names span the forms the modelled
/// `Name` pattern admits (`^[a-zA-Z0-9\s_.,-]+`): letters, digits, mixed case, a space, a
/// dot, a comma and an underscore, plus one name that is another's prefix and one that is
/// another's case variant.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a hyphenated name",
            name: "runbook-escalation",
            content_type: "text/plain",
            extra: json!({
                "title": "Escalation runbook",
                "metadata": { "source": "crm" },
                "tags": { "team": "support" },
                "overrideLinkOutUri": "https://example.test/articles/1",
            }),
        },
        Row {
            case: "a name that is another's prefix",
            name: "runbook",
            content_type: "text/html",
            extra: json!({}),
        },
        Row {
            case: "a name carrying a space and a dot",
            name: "Billing FAQ v2.1",
            content_type: "text/csv",
            extra: json!({ "metadata": { "locale": "en-GB" } }),
        },
        Row {
            case: "a name differing from another only in case",
            name: "BILLING FAQ v2.1",
            content_type: "application/pdf",
            extra: json!({ "tags": { "tier": "gold" } }),
        },
        Row {
            case: "a name carrying an underscore and digits",
            name: "policy_2026_01",
            content_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            extra: json!({ "title": "A policy" }),
        },
        Row {
            case: "a name carrying a comma",
            name: "returns, refunds and credits",
            content_type: "application/x.wisdom-json;source=salesforce",
            extra: json!({ "metadata": { "table": "kb" } }),
        },
        Row {
            case: "a name of digits alone",
            name: "20260101",
            content_type: "application/x.wisdom-json;source=zendesk",
            extra: json!({}),
        },
    ]
}

/// One created content, as the create reported it.
struct Created {
    case: &'static str,
    name: &'static str,
    content_id: String,
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
            name: row.name,
            content_id: reply.string("/content/contentId"),
            data: reply.object("/content"),
        });
    }
    created
}

/// RULE: a search reports the contents of the knowledge base the route names that satisfy
/// the expression, as the stored records — so a `NAME`/`EQUALS` filter answers with the one
/// content bearing that name, member for member the summary the plain listing of the same
/// collection reports.
#[tokio::test]
async fn a_name_filter_reports_the_one_stored_content_bearing_that_name() {
    let emulator = Emulator::new();
    let (knowledge_base_id, knowledge_base_arn) =
        emulator.knowledge_base("search-kb", "CUSTOM").await;
    let created = create_contents(&emulator, &knowledge_base_id).await;

    // Past the span the create's ingestion is owed over, so every record is in its
    // terminal status and the search and the listing are comparable member by member.
    tokio::time::sleep(state::CONTENT_ACTIVATION_DELAY + Duration::from_millis(50)).await;

    let listed = emulator.list(&knowledge_base_id).await;
    assert_eq!(
        listed.status, 200,
        "case \"the plain listing\": expected HTTP 200, got {} with {}",
        listed.status, listed.body
    );
    let listed_summaries = listed.array("/contentSummaries");

    for entry in &created {
        let case = entry.case;
        let reply = emulator
            .search(&knowledge_base_id, by_names(&[entry.name]), "")
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        let matches = reply.array("/contentSummaries");
        assert_eq!(
            matches.len(),
            1,
            "case {case:?}: a name is unique within a knowledge base, so exactly one \
             content matches, got {}",
            reply.body
        );
        assert_eq!(
            matches[0].pointer("/contentId").and_then(Value::as_str),
            Some(entry.content_id.as_str()),
            "case {case:?}: expected the content the create persisted under that name, \
             got {}",
            reply.body
        );
        assert!(
            reply.body.pointer("/nextToken").is_none(),
            "case {case:?}: a page that ends the results carries no nextToken, got {}",
            reply.body
        );

        // The match is the stored record, not a separately computed answer: it equals the
        // summary the plain listing of the same collection reports.
        let listed_match = listed_summaries
            .iter()
            .find(|summary| {
                summary.pointer("/contentId").and_then(Value::as_str)
                    == Some(entry.content_id.as_str())
            })
            .unwrap_or_else(|| panic!("case {case:?}: the listing must hold the created content"));
        assert_eq!(
            &matches[0], listed_match,
            "case {case:?}: a match must be the stored summary the listing reports ({}), \
             got {}",
            listed_match, matches[0]
        );
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
                matches[0].pointer(&format!("/{member}")),
                entry.data.get(member),
                "case {case:?}: member {member:?} must be the one the create persisted \
                 ({:?}), got {:?}",
                entry.data.get(member),
                matches[0].pointer(&format!("/{member}"))
            );
        }
        assert_eq!(
            matches[0]
                .pointer("/knowledgeBaseArn")
                .and_then(Value::as_str),
            Some(knowledge_base_arn.as_str()),
            "case {case:?}: a match reports its knowledge base's own ARN, got {:?}",
            matches[0].pointer("/knowledgeBaseArn")
        );
        assert_eq!(
            matches[0].pointer("/status").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {case:?}: after the activation span a match reports the terminal \
             status, got {:?}",
            matches[0].pointer("/status")
        );
    }

    // Either label form of the knowledge base addresses the same collection.
    let by_arn = emulator
        .search(&knowledge_base_arn, by_names(&[created[0].name]), "")
        .await;
    assert_eq!(
        by_arn.matched_ids(),
        vec![created[0].content_id.clone()],
        "case \"the knowledge base addressed by ARN\": every label form of one knowledge \
         base answers alike, got {}",
        by_arn.body
    );
}

/// RULE: a filter compares the selected field's value with its own under its operator, so
/// `EQUALS` accepts the value itself and nothing that merely resembles it.
#[tokio::test]
async fn equals_selects_the_value_itself_and_nothing_that_resembles_it() {
    let emulator = Emulator::new();
    let (knowledge_base_id, _) = emulator.knowledge_base("equality-kb", "CUSTOM").await;
    let created = create_contents(&emulator, &knowledge_base_id).await;
    let held = created[0].name;

    // Each row is a value that is NOT the stored name, however close: the stored names
    // include one that is another's prefix and one that differs only in case, so a
    // resemblance match would report the wrong content for these.
    let table: [(&str, String); 7] = [
        ("a prefix of a stored name", "runbook-".to_string()),
        ("a suffix of a stored name", "escalation".to_string()),
        ("a stored name with trailing space", format!("{held} ")),
        ("a stored name with leading space", format!(" {held}")),
        ("a stored name upper-cased", held.to_uppercase()),
        (
            "a stored name with one letter dropped",
            held[1..].to_string(),
        ),
        (
            "a name nothing was created under",
            "absent-content".to_string(),
        ),
    ];

    for (case, value) in table {
        let reply = emulator
            .search(&knowledge_base_id, by_names(&[value.as_str()]), "")
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a search matching nothing is an empty page, not a refusal, \
             got {} with {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.matched_ids(),
            Vec::<String>::new(),
            "case {case:?}: {value:?} is not a stored name, so nothing matches, got {}",
            reply.body
        );
        assert!(
            reply.body.pointer("/contentSummaries").is_some(),
            "case {case:?}: contentSummaries is required, so an empty result still \
             carries the list, got {}",
            reply.body
        );
    }

    // The two names differing only in case each select their OWN content, which is what
    // makes the comparison an equality rather than a fold.
    for entry in created
        .iter()
        .filter(|entry| entry.name.eq_ignore_ascii_case("Billing FAQ v2.1"))
    {
        let reply = emulator
            .search(&knowledge_base_id, by_names(&[entry.name]), "")
            .await;
        assert_eq!(
            reply.matched_ids(),
            vec![entry.content_id.clone()],
            "case {:?}: {:?} selects the content stored under exactly that name, got {}",
            entry.case,
            entry.name,
            reply.body
        );
    }
}

/// RULE: a search expression is the conjunction of its filters — every filter must accept
/// a content for it to be reported — so an expression carrying no filter selects the whole
/// collection.
#[tokio::test]
async fn an_expression_conjoins_its_filters() {
    let emulator = Emulator::new();
    let (knowledge_base_id, _) = emulator.knowledge_base("conjunction-kb", "CUSTOM").await;
    let created = create_contents(&emulator, &knowledge_base_id).await;
    let every_id = {
        let mut ids: Vec<String> = created
            .iter()
            .map(|entry| entry.content_id.clone())
            .collect();
        ids.sort();
        ids
    };
    let first = created[0].name;
    let second = created[2].name;
    let only_first = vec![created[0].content_id.clone()];

    let table: [(&str, Value, Vec<String>); 5] = [
        ("no filter at all", by_names(&[]), every_id.clone()),
        (
            "one filter naming a stored content",
            by_names(&[first]),
            only_first.clone(),
        ),
        (
            "the same filter twice",
            by_names(&[first, first]),
            only_first.clone(),
        ),
        (
            "two filters naming different contents",
            by_names(&[first, second]),
            Vec::new(),
        ),
        (
            "three filters, one of them matching nothing",
            by_names(&[first, first, "absent-content"]),
            Vec::new(),
        ),
    ];

    for (case, expression, expected) in table {
        let reply = emulator
            .search(&knowledge_base_id, expression, "?maxResults=100")
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.matched_ids(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?} from {}",
            reply.matched_ids(),
            reply.body
        );
    }
}

/// RULE: the matches are ordered by `contentId` and the page size bounds a page, the
/// continuation token naming where the next page resumes — over every page size the model
/// allows (`maxResults` is `range 1..100`), with a value outside that range and a token
/// this service never minted both refused.
#[tokio::test]
async fn the_page_size_and_the_continuation_token_walk_every_match() {
    let emulator = Emulator::new();
    let (knowledge_base_id, _) = emulator.knowledge_base("paging-kb", "CUSTOM").await;
    let created = create_contents(&emulator, &knowledge_base_id).await;
    let every_id = {
        let mut ids: Vec<String> = created
            .iter()
            .map(|entry| entry.content_id.clone())
            .collect();
        ids.sort();
        ids
    };

    // No page size, both published bounds of `maxResults` and the sizes in between.
    for page_size in [None, Some(1), Some(2), Some(3), Some(7), Some(100)] {
        let case = format!("maxResults {page_size:?}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..=every_id.len() {
            let mut query: Vec<String> = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(from) = &token {
                query.push(format!("nextToken={from}"));
            }
            let query = if query.is_empty() {
                String::new()
            } else {
                format!("?{}", query.join("&"))
            };
            let page = emulator
                .search(&knowledge_base_id, by_names(&[]), &query)
                .await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected HTTP 200, got {} with {}",
                page.status, page.body
            );
            let ids = page.matched_ids();
            let mut sorted = ids.clone();
            sorted.sort();
            assert_eq!(
                ids, sorted,
                "case {case:?}: the matches are ordered by contentId, got {ids:?}"
            );
            if let Some(size) = page_size {
                assert!(
                    ids.len() <= size,
                    "case {case:?}: a page holds at most maxResults matches, got {}",
                    ids.len()
                );
            }
            walked.extend(ids);
            token = page
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
            "case {case:?}: the walk must end without a nextToken, got {token:?}"
        );
        assert_eq!(
            walked, every_id,
            "case {case:?}: every match is reported exactly once across the pages, got \
             {walked:?}"
        );
    }

    // One past each published bound is outside `maxResults`' modelled range, and a token
    // this service never minted names no position.
    for (case, query, expected) in [
        ("maxResults at the lower bound", "?maxResults=1", 200),
        ("maxResults at the upper bound", "?maxResults=100", 200),
        ("maxResults one below the lower bound", "?maxResults=0", 400),
        ("maxResults one past the upper bound", "?maxResults=101", 400),
        ("a nextToken this service never minted", "?nextToken=not-a-cursor", 400),
    ] {
        let reply = emulator
            .search(&knowledge_base_id, by_names(&[]), query)
            .await;
        assert_eq!(
            reply.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with {}",
            reply.status, reply.body
        );
    }
}

/// RULE: a label this account holds no such resource under is the modelled
/// `ResourceNotFoundException` (HTTP 404), stating the sentence this endpoint words for
/// that resource family and naming the requested label in the member the model provides
/// for it — and a content is reachable only through the knowledge base it lives in, so
/// another knowledge base's content is never a match.
#[tokio::test]
async fn a_label_nothing_is_held_under_is_the_modelled_not_found() {
    let emulator = Emulator::new();
    let (knowledge_base_id, _) = emulator.knowledge_base("not-found-search-kb", "CUSTOM").await;
    let (other_id, other_arn) = emulator
        .knowledge_base("not-found-search-other-kb", "CUSTOM")
        .await;
    let created = create_contents(&emulator, &knowledge_base_id).await;
    let absent_arn = format!(
        "arn:aws:wisdom:{REGION}:{}:knowledge-base/{ABSENT_UUID}",
        engine::EMULATOR_ACCOUNT_ID
    );

    for (case, knowledge_base_label) in [
        (
            "a knowledge base id nothing was created under",
            ABSENT_UUID.to_string(),
        ),
        (
            "a knowledge base ARN nothing was created under",
            absent_arn.clone(),
        ),
    ] {
        let reply = emulator
            .search(&knowledge_base_label, by_names(&[created[0].name]), "")
            .await;
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
            state::MESSAGE_KNOWLEDGE_BASE_NOT_FOUND,
            "case {case:?}: expected the sentence this endpoint words for that resource, \
             got {:?}",
            reply.string("/message")
        );
        assert_eq!(
            reply.string("/resourceName"),
            knowledge_base_label,
            "case {case:?}: the requested label travels in resourceName, got {:?}",
            reply.string("/resourceName")
        );
    }

    // A knowledge base this account holds, but not the one the content lives in, matches
    // nothing — by either of its label forms.
    for (case, label_form) in [
        ("another knowledge base by id", other_id.clone()),
        ("another knowledge base by ARN", other_arn.clone()),
    ] {
        for entry in &created {
            let reply = emulator
                .search(&label_form, by_names(&[entry.name]), "")
                .await;
            assert_eq!(
                reply.status, 200,
                "case \"{case}, {}\": expected HTTP 200, got {} with {}",
                entry.name, reply.status, reply.body
            );
            assert_eq!(
                reply.matched_ids(),
                Vec::<String>::new(),
                "case \"{case}, {}\": a content of another knowledge base is not a \
                 match, got {}",
                entry.name,
                reply.body
            );
        }
    }
}

/// RULE: a reporting operation changes nothing a later caller sees — repeated searches of
/// an unchanged collection answer alike, the plain listing is untouched, and so is the
/// asset locator of a matched content.
#[tokio::test]
async fn the_search_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let (knowledge_base_id, _) = emulator.knowledge_base("read-only-kb", "CUSTOM").await;
    let created = create_contents(&emulator, &knowledge_base_id).await;
    tokio::time::sleep(state::CONTENT_ACTIVATION_DELAY + Duration::from_millis(50)).await;

    let listed_before = emulator.list(&knowledge_base_id).await;
    for entry in &created {
        let case = entry.case;
        let before = emulator.content(&knowledge_base_id, &entry.content_id).await;
        let first = emulator
            .search(&knowledge_base_id, by_names(&[entry.name]), "")
            .await;
        let second = emulator
            .search(&knowledge_base_id, by_names(&[entry.name]), "")
            .await;
        assert_eq!(
            first.array("/contentSummaries"),
            second.array("/contentSummaries"),
            "case {case:?}: two searches of an unchanged collection answer alike, got {} \
             then {}",
            first.body,
            second.body
        );
        let after = emulator.content(&knowledge_base_id, &entry.content_id).await;
        for locator in ["url", "urlExpiry", "revisionId", "status"] {
            assert_eq!(
                after.object("/content").get(locator),
                before.object("/content").get(locator),
                "case {case:?}: a search must leave {locator:?} as it was, got {:?}",
                after.object("/content").get(locator)
            );
        }
    }
    let listed_after = emulator.list(&knowledge_base_id).await;
    assert_eq!(
        listed_after.array("/contentSummaries"),
        listed_before.array("/contentSummaries"),
        "case \"the plain listing after the searches\": a search persists nothing, got {}",
        listed_after.body
    );
}

/// RULE: the status a resource the service reports in a non-terminal status moves to is
/// owed by the CLOCK, and a search reports the status the clock owes at the instant of the
/// search, writing nothing to make it so.
#[tokio::test]
async fn a_match_reports_the_status_the_clock_owes() {
    let emulator = Emulator::new();
    let (knowledge_base_id, _) = emulator.knowledge_base("clock-search-kb", "CUSTOM").await;
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

    let during = emulator
        .search(&knowledge_base_id, by_names(&["activating-content"]), "")
        .await;
    assert_eq!(
        during.matched_ids(),
        vec![content_id.clone()],
        "case \"straight after the create\": the created content is already a match, got {}",
        during.body
    );
    assert_eq!(
        during.string("/contentSummaries/0/status"),
        "CREATE_IN_PROGRESS",
        "case \"straight after the create\": expected the transient status, got {}",
        during.body
    );

    tokio::time::sleep(state::CONTENT_ACTIVATION_DELAY + Duration::from_millis(50)).await;

    let after = emulator
        .search(&knowledge_base_id, by_names(&["activating-content"]), "")
        .await;
    assert_eq!(
        after.string("/contentSummaries/0/status"),
        "ACTIVE",
        "case \"after the activation span\": expected the terminal status, got {}",
        after.body
    );
}

/// What one row of the write table writes to a content between its create and the search.
enum Write {
    /// Members written through `UpdateContent`.
    Update(Value),
    /// A replacement asset, named by the media type its upload is started for and pointed
    /// at through `UpdateContent`'s `uploadId`.
    ReplaceAsset(&'static str),
    /// Tags written through `TagResource` on the content's own ARN.
    Tag(Value),
}

/// One row of the write table: a case name, the write it lands, the summary members the
/// search must report once it has landed, and whether it is one of the content's own
/// revisions.
struct WriteRow {
    case: &'static str,
    write: Write,
    expected: Value,
    moves_revision: bool,
}

/// The writes a later search's answer must follow, one row per member a write of a content
/// can move: the whole set `UpdateContent` documents it writes
/// (`smithy_query(query="operation",operation="UpdateContent")`: `title`, `metadata`,
/// `overrideLinkOutUri`/`removeOverrideLinkOutUri` and `uploadId`), two of them written
/// together, a map replaced by an empty one at its own published lower bound (`metadata`,
/// `length 0..10`), and the `tags` map the OTHER write path of the same record reaches.
fn write_rows() -> Vec<WriteRow> {
    vec![
        WriteRow {
            case: "a replaced title",
            write: Write::Update(json!({ "title": "The title the write gave" })),
            expected: json!({ "title": "The title the write gave" }),
            moves_revision: true,
        },
        WriteRow {
            case: "a replaced metadata map",
            write: Write::Update(json!({
                "metadata": { "locale": "fr-FR", "system": "servicenow" },
            })),
            expected: json!({ "metadata": { "locale": "fr-FR", "system": "servicenow" } }),
            moves_revision: true,
        },
        WriteRow {
            case: "metadata replaced by an empty map",
            write: Write::Update(json!({ "metadata": {} })),
            expected: json!({ "metadata": {} }),
            moves_revision: true,
        },
        WriteRow {
            case: "a title and a metadata map written together",
            write: Write::Update(json!({
                "title": "Both members at once",
                "metadata": { "table": "kb" },
            })),
            expected: json!({
                "title": "Both members at once",
                "metadata": { "table": "kb" },
            }),
            moves_revision: true,
        },
        WriteRow {
            case: "the override link-out URI removed",
            write: Write::Update(json!({ "removeOverrideLinkOutUri": true })),
            expected: json!({}),
            moves_revision: true,
        },
        WriteRow {
            case: "a replaced asset, of another media type",
            write: Write::ReplaceAsset("text/html"),
            expected: json!({ "contentType": "text/html" }),
            moves_revision: true,
        },
        WriteRow {
            case: "a tag written onto the content after its create",
            write: Write::Tag(json!({ "team": "billing" })),
            expected: json!({ "tags": { "owner": "support", "team": "billing" } }),
            moves_revision: false,
        },
    ]
}

/// Create one content under `knowledge_base_id`, named `name`, carrying a value in every
/// member a row of [`write_rows`] goes on to replace.
async fn create_content_named(emulator: &Emulator, knowledge_base_id: &str, name: &str) -> Reply {
    let upload_id = emulator
        .written_upload(knowledge_base_id, "text/plain", b"the asset the create finalized")
        .await;
    let reply = emulator
        .send(
            "POST",
            &format!("/knowledgeBases/{knowledge_base_id}/contents"),
            Some(json!({
                "name": name,
                "uploadId": upload_id,
                "title": "The title the create gave",
                "metadata": { "source": "crm" },
                "tags": { "owner": "support" },
                "overrideLinkOutUri": "https://example.test/articles/created",
            })),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: expected HTTP 200 from the create, got {} with {}",
        reply.status, reply.body
    );
    reply
}

/// RULE: a reporting operation answers out of the stored record as it stands at the call,
/// so a match carries what the LAST write to that content persisted — never the values the
/// create first gave it.
#[tokio::test]
async fn a_match_reports_the_record_as_the_last_write_left_it() {
    let emulator = Emulator::new();
    let (knowledge_base_id, _) = emulator
        .knowledge_base("rewritten-search-kb", "CUSTOM")
        .await;

    // One content per row, all created before the span the create's ingestion is owed
    // over, so a single wait leaves every record in its terminal status and the search and
    // the read of one content are comparable member by member.
    let rows = write_rows();
    let mut contents = Vec::new();
    for index in 0..rows.len() {
        let name = format!("rewritten content {index}");
        let created = create_content_named(&emulator, &knowledge_base_id, &name).await;
        contents.push((
            name,
            created.string("/content/contentId"),
            created.string("/content/contentArn"),
            created.string("/content/revisionId"),
        ));
    }
    tokio::time::sleep(state::CONTENT_ACTIVATION_DELAY + Duration::from_millis(50)).await;

    for (row, (name, content_id, content_arn, revision_before)) in rows.iter().zip(&contents) {
        let case = row.case;
        let written = match &row.write {
            Write::Update(members) => {
                emulator
                    .send(
                        "POST",
                        &format!("/knowledgeBases/{knowledge_base_id}/contents/{content_id}"),
                        Some(members.clone()),
                    )
                    .await
            }
            Write::ReplaceAsset(content_type) => {
                let upload_id = emulator
                    .written_upload(&knowledge_base_id, content_type, b"the replacement asset")
                    .await;
                emulator
                    .send(
                        "POST",
                        &format!("/knowledgeBases/{knowledge_base_id}/contents/{content_id}"),
                        Some(json!({ "uploadId": upload_id })),
                    )
                    .await
            }
            Write::Tag(tags) => {
                emulator
                    .send(
                        "POST",
                        &format!("/tags/{}", encode_label(content_arn)),
                        Some(json!({ "tags": tags })),
                    )
                    .await
            }
        };
        assert!(
            written.status == 200 || written.status == 204,
            "case {case:?}: expected the write to be served, got {} with {}",
            written.status,
            written.body
        );

        let reply = emulator
            .search(&knowledge_base_id, by_names(&[name.as_str()]), "")
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        let matches = reply.array("/contentSummaries");
        assert_eq!(
            matches.len(),
            1,
            "case {case:?}: a write of these members leaves the content matching its own \
             name, got {}",
            reply.body
        );
        let matched = matches[0].as_object().cloned().unwrap_or_default();

        // Every member the write moved is reported with the value the write persisted.
        for (member, value) in row.expected.as_object().expect("the expected members") {
            assert_eq!(
                matched.get(member),
                Some(value),
                "case {case:?}: member {member:?} must be the one the write persisted \
                 ({value}), got {:?}",
                matched.get(member)
            );
        }

        // And the match is that one record rather than a separately kept copy of it: it
        // equals the read of the same content with the asset locator members dropped — the
        // ones `ContentData` carries and `ContentSummary` does not
        // (`smithy_query(query="shape",shape="ContentSummary,ContentData")`).
        let fetched = emulator.content(&knowledge_base_id, content_id).await;
        let mut stored = fetched.object("/content");
        for locator in ["linkOutUri", "url", "urlExpiry"] {
            stored.remove(locator);
        }
        assert_eq!(
            matched, stored,
            "case {case:?}: a match must be the stored record a read of the same content \
             reports ({stored:?}), got {matched:?}"
        );

        // A write of the content's own members mints the next revision of it, and the
        // search reports that one, not the revision the create first persisted.
        if row.moves_revision {
            let reported = matched
                .get("revisionId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert_ne!(
                reported, revision_before,
                "case {case:?}: a landed write moves the revision the search reports, got \
                 {reported:?}"
            );
            assert_eq!(
                Some(reported),
                written
                    .body
                    .pointer("/content/revisionId")
                    .and_then(Value::as_str),
                "case {case:?}: the search must report the revision the write minted ({}), \
                 got {reported:?}",
                written.body
            );
        }
    }
}
