// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetRecommendations` on the Amazon Q in Connect endpoint,
//! driven over HTTP through the same router the production emulator mounts.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! * `GetRecommendations` REPORTS the recommendations persisted on the session the route
//!   names — every recommendation it answers with, and every trigger, is the stored
//!   record, oldest first — in either `UuidOrArn` label form;
//! * a session with nothing available answers HTTP 200 with an EMPTY `recommendations`
//!   list, which is what the modelled contract says a wait that expires with no
//!   recommendation returns;
//! * `maxResults` bounds the page and `recommendationType` filters it to that type, for
//!   every page size and every type the model declares;
//! * the operation is `@readonly`: reading does not consume the feed, so repeating a
//!   read answers alike;
//! * a label this account holds no ASSISTANT under is `ResourceNotFoundException` (HTTP
//!   404), while a session the named assistant does not hold has nothing available and
//!   answers the same successful empty list (the recorded unit
//!   `wisdom.get-recommendations-nonexistent-session-rejected` shows the endpoint
//!   answering exactly that);
//! * `waitTimeSeconds` is the wait the caller asked for: a short poll (0) answers at
//!   once, a longer wait returns as soon as a recommendation is available and otherwise
//!   waits out its wait and answers empty;
//! * a `nextChunkToken` this service never issued is refused as a `ValidationException`
//!   (HTTP 400) rather than answered with a page a client would read as the
//!   continuation it asked for;
//! * the feed is bounded, so publishing past the ceiling forgets the OLDEST
//!   recommendation rather than growing the store without limit.
//!
//! The recommendations are put on the session's feed through the state seam
//! `RecommendationFeed::publish`: the service makes a recommendation available out of
//! band (from the contact's detected intent), and no implemented operation of this model
//! publishes one — `GetRecommendations` only reports what the session holds. Everything
//! else goes over the wire, so a response that is not computed from persisted state
//! cannot pass.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_qconnect_wisdomservice::ops::types::{
    ContentReference, Document, DocumentText, QueryRecommendationTriggerData, RecommendationData,
    RecommendationTrigger, RecommendationTriggerData,
};
use service_com_amazonaws_qconnect_wisdomservice::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for.
const REGION: &str = "us-east-1";

/// A UUID no create ever mints, for the not-found assertions.
const ABSENT_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// The `RecommendationType` values this suite publishes, taken from the model's value
/// set (`smithy_query(query="shape",shape="RecommendationType")`, which declares
/// `KNOWLEDGE_CONTENT`, `GENERATIVE_RESPONSE`, `GENERATIVE_ANSWER`, `DETECTED_INTENT`
/// and the chunked variants). Three distinct values are enough to tell a filtered page
/// from an unfiltered one.
const RECOMMENDATION_TYPES: [&str; 3] = ["KNOWLEDGE_CONTENT", "GENERATIVE_RESPONSE", "DETECTED_INTENT"];

/// A `RecommendationType` the model declares that this suite never publishes, so a call
/// filtering on it is served an empty page.
const UNPUBLISHED_TYPE: &str = "SUGGESTED_MESSAGE";

/// Every `RelevanceLevel` the model declares
/// (`smithy_query(query="shape",shape="RelevanceLevel")`).
const RELEVANCE_LEVELS: [&str; 3] = ["HIGH", "MEDIUM", "LOW"];

/// Every `RecommendationSourceType` the model declares
/// (`smithy_query(query="shape",shape="RecommendationSourceType")`).
const SOURCE_TYPES: [&str; 3] = ["ISSUE_DETECTION", "RULE_EVALUATION", "OTHER"];

/// Both `RecommendationTriggerType` values the model declares
/// (`smithy_query(query="shape",shape="RecommendationTriggerType")`).
const TRIGGER_TYPES: [&str; 2] = ["QUERY", "GENERATIVE"];

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

    fn string(&self, pointer: &str) -> String {
        self.field(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn recommendations(&self) -> Vec<Value> {
        self.field("/recommendations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    fn triggers(&self) -> Vec<Value> {
        self.field("/triggers")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }
}

/// A `UuidOrArn` label as it travels in a URL: an ARN's slashes are percent-encoded,
/// which is what the model's "URLs cannot contain the ARN" note is about.
fn encode_label(label: &str) -> String {
    label.replace('/', "%2F")
}

/// What one published recommendation is made of, so the assertions compare the wire
/// answer against the STORED record rather than against a literal.
#[derive(Debug, Clone)]
struct Published {
    recommendation_id: String,
    trigger_id: String,
    excerpt: String,
    title: String,
    query_text: String,
    knowledge_base_id: String,
    content_id: String,
    relevance_score: f64,
    relevance_level: &'static str,
    recommendation_type: &'static str,
    trigger_type: &'static str,
    source: &'static str,
}

/// An emulator with just this service mounted on a fresh engine. The engine is kept so
/// the suite can put recommendations on a session's feed.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes by the
        // SigV4 credential scope below, which is also where an ARN's region comes from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/wisdom/aws4_request, \
             SignedHeaders=host, Signature=deadbeef"
        );
        let builder = http::Request::builder()
            .method(method)
            .uri(format!("https://wisdom.{REGION}.amazonaws.com{uri}"))
            .header("host", format!("wisdom.{REGION}.amazonaws.com"))
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

    /// An assistant, returned as (id, ARN).
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

    /// A session under `assistant_label`, returned as (id, ARN).
    async fn session(&self, assistant_label: &str, name: &str) -> (String, String) {
        let reply = self
            .send(
                "POST",
                &format!("/assistants/{}/sessions", encode_label(assistant_label)),
                Some(json!({ "name": name })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"session {name}\": expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        (
            reply.string("/session/sessionId"),
            reply.string("/session/sessionArn"),
        )
    }

    /// `GetRecommendations` on one (assistant label, session label) pair, with the query
    /// members the row names.
    async fn get_recommendations(
        &self,
        assistant_label: &str,
        session_label: &str,
        query: &[(&str, String)],
    ) -> Reply {
        let suffix = if query.is_empty() {
            String::new()
        } else {
            format!(
                "?{}",
                query
                    .iter()
                    .map(|(name, value)| format!("{name}={value}"))
                    .collect::<Vec<String>>()
                    .join("&")
            )
        };
        self.send(
            "GET",
            &format!(
                "/assistants/{}/sessions/{}/recommendations{suffix}",
                encode_label(assistant_label),
                encode_label(session_label)
            ),
            None,
        )
        .await
    }

    fn publish(&self, assistant_label: &str, session_label: &str, index: usize) -> Published {
        publish(&self.engine, assistant_label, session_label, index)
    }
}

/// The query string for a short poll of a whole page, which most rows want.
fn short_poll() -> Query {
    vec![("waitTimeSeconds", "0".to_string())]
}

/// One call's query string: the `@httpQuery` members a row names, in order.
type Query = Vec<(&'static str, String)>;

/// Make one recommendation available on a session's feed, through the state seam a
/// producer of recommendations writes to.
///
/// Takes the engine rather than the whole emulator, so a task that only publishes (the
/// mid-wait publisher below) needs nothing else.
fn publish(
    engine: &engine::InMemoryEngine,
    assistant_label: &str,
    session_label: &str,
    index: usize,
) -> Published {
    let published = Published {
        recommendation_id: format!("recommendation-{index}"),
        trigger_id: trigger_uuid(index),
        excerpt: format!("excerpt of recommendation {index}"),
        title: format!("title {index}"),
        query_text: format!("what the contact asked, take {index}"),
        knowledge_base_id: resource_uuid(0x10 + index),
        content_id: resource_uuid(0x40 + index),
        // A score that differs per row, so a reported score cannot be a constant that
        // happens to match.
        relevance_score: 0.125 * (index as f64 + 1.0),
        relevance_level: RELEVANCE_LEVELS[index % RELEVANCE_LEVELS.len()],
        recommendation_type: RECOMMENDATION_TYPES[index % RECOMMENDATION_TYPES.len()],
        trigger_type: TRIGGER_TYPES[index % TRIGGER_TYPES.len()],
        source: SOURCE_TYPES[index % SOURCE_TYPES.len()],
    };
    let store = state::service_state(engine).expect("this service's store");
    let mut store = store.lock();
    let assistant = store
        .assistant_mut(assistant_label)
        .expect("an assistant to publish under");
    let session = assistant
        .sessions
        .get_mut(state::resolve_id(session_label))
        .expect("a session to publish onto");
    session.recommendations.publish(
        RecommendationData {
            recommendation_id: published.recommendation_id.clone(),
            document: Some(Document {
                content_reference: ContentReference {
                    knowledge_base_arn: None,
                    knowledge_base_id: Some(published.knowledge_base_id.clone()),
                    content_arn: None,
                    content_id: Some(published.content_id.clone()),
                    source_url: None,
                    reference_type: None,
                },
                title: Some(DocumentText {
                    text: Some(published.title.clone()),
                    highlights: None,
                }),
                excerpt: Some(DocumentText {
                    text: Some(published.excerpt.clone()),
                    highlights: None,
                }),
            }),
            relevance_score: published.relevance_score,
            relevance_level: Some(published.relevance_level.to_string()),
            r#type: Some(published.recommendation_type.to_string()),
            data: None,
        },
        Some(RecommendationTrigger {
            id: published.trigger_id.clone(),
            r#type: published.trigger_type.to_string(),
            source: published.source.to_string(),
            data: RecommendationTriggerData::Query(QueryRecommendationTriggerData {
                text: Some(published.query_text.clone()),
            }),
            recommendation_ids: vec![published.recommendation_id.clone()],
        }),
    );
    published
}

/// A uuid-shaped id for a knowledge base or content reference: the modelled members are
/// `Uuid` (`^[a-f0-9]{8}-…$`), so the published record carries that form.
fn resource_uuid(index: usize) -> String {
    format!("{index:08x}-0000-4000-8000-000000000000")
}

/// A uuid-shaped trigger id — `RecommendationTrigger.id` is modelled `Uuid`.
fn trigger_uuid(index: usize) -> String {
    format!("{index:08x}-1111-4111-8111-111111111111")
}

/// Every reported recommendation is the stored record, in the order the service made
/// them available, and the reported triggers are the triggers of the recommendations IN
/// the page.
fn assert_reports(case: &str, reply: &Reply, expected: &[&Published]) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {}",
        reply.status, reply.body
    );
    let reported = reply.recommendations();
    assert_eq!(
        reported.len(),
        expected.len(),
        "case {case:?}: expected {} recommendation(s), got {}",
        expected.len(),
        reply.body
    );
    for (position, (actual, published)) in reported.iter().zip(expected).enumerate() {
        assert_eq!(
            actual.pointer("/recommendationId").and_then(Value::as_str),
            Some(published.recommendation_id.as_str()),
            "case {case:?}: unexpected recommendationId at position {position}, got {actual}"
        );
        assert_eq!(
            actual
                .pointer("/document/excerpt/text")
                .and_then(Value::as_str),
            Some(published.excerpt.as_str()),
            "case {case:?}: unexpected excerpt at position {position}, got {actual}"
        );
        assert_eq!(
            actual
                .pointer("/document/title/text")
                .and_then(Value::as_str),
            Some(published.title.as_str()),
            "case {case:?}: unexpected title at position {position}, got {actual}"
        );
        assert_eq!(
            actual
                .pointer("/document/contentReference/knowledgeBaseId")
                .and_then(Value::as_str),
            Some(published.knowledge_base_id.as_str()),
            "case {case:?}: unexpected knowledgeBaseId at position {position}, got {actual}"
        );
        assert_eq!(
            actual
                .pointer("/document/contentReference/contentId")
                .and_then(Value::as_str),
            Some(published.content_id.as_str()),
            "case {case:?}: unexpected contentId at position {position}, got {actual}"
        );
        assert!(
            actual
                .pointer("/relevanceScore")
                .and_then(Value::as_f64)
                .is_some_and(|score| (score - published.relevance_score).abs() < f64::EPSILON),
            "case {case:?}: expected relevanceScore {} at position {position}, got {actual}",
            published.relevance_score
        );
        assert_eq!(
            actual.pointer("/relevanceLevel").and_then(Value::as_str),
            Some(published.relevance_level),
            "case {case:?}: unexpected relevanceLevel at position {position}, got {actual}"
        );
        assert_eq!(
            actual.pointer("/type").and_then(Value::as_str),
            Some(published.recommendation_type),
            "case {case:?}: unexpected type at position {position}, got {actual}"
        );
    }

    let triggers = reply.triggers();
    assert_eq!(
        triggers.len(),
        expected.len(),
        "case {case:?}: expected one trigger per reported recommendation, got {}",
        reply.body
    );
    for (position, (actual, published)) in triggers.iter().zip(expected).enumerate() {
        assert_eq!(
            actual.pointer("/id").and_then(Value::as_str),
            Some(published.trigger_id.as_str()),
            "case {case:?}: unexpected trigger id at position {position}, got {actual}"
        );
        assert_eq!(
            actual.pointer("/type").and_then(Value::as_str),
            Some(published.trigger_type),
            "case {case:?}: unexpected trigger type at position {position}, got {actual}"
        );
        assert_eq!(
            actual.pointer("/source").and_then(Value::as_str),
            Some(published.source),
            "case {case:?}: unexpected trigger source at position {position}, got {actual}"
        );
        assert_eq!(
            actual.pointer("/data/query/text").and_then(Value::as_str),
            Some(published.query_text.as_str()),
            "case {case:?}: unexpected trigger data at position {position}, got {actual}"
        );
        assert_eq!(
            actual
                .pointer("/recommendationIds")
                .and_then(Value::as_array)
                .map(|ids| ids.iter().filter_map(Value::as_str).collect::<Vec<&str>>()),
            Some(vec![published.recommendation_id.as_str()]),
            "case {case:?}: unexpected trigger recommendationIds at position {position}, got {actual}"
        );
    }
}

/// A session with nothing available answers HTTP 200 with an empty `recommendations`
/// list — in either label form and at every page size the model allows, including both
/// published `maxResults` bounds.
#[tokio::test]
async fn a_session_with_nothing_available_answers_an_empty_list() {
    let emulator = Emulator::new();
    let (assistant_id, assistant_arn) = emulator.assistant("empty-feed assistant").await;
    let (session_id, session_arn) = emulator.session(&assistant_id, "empty-feed session").await;

    let labels: Vec<(&str, &str, &str)> = vec![
        ("id and id", assistant_id.as_str(), session_id.as_str()),
        ("arn and id", assistant_arn.as_str(), session_id.as_str()),
        ("id and arn", assistant_id.as_str(), session_arn.as_str()),
        ("arn and arn", assistant_arn.as_str(), session_arn.as_str()),
    ];
    // `maxResults` is modelled `range 1..100`: no page size, both bounds and one size
    // inside.
    let page_sizes: [Option<i64>; 4] = [None, Some(1), Some(7), Some(100)];

    for (labels_case, assistant_label, session_label) in labels {
        for max_results in page_sizes {
            let case = format!("{labels_case}, maxResults {max_results:?}");
            let mut query = short_poll();
            if let Some(max_results) = max_results {
                query.push(("maxResults", max_results.to_string()));
            }
            let reply = emulator
                .get_recommendations(assistant_label, session_label, &query)
                .await;
            assert_reports(&case, &reply, &[]);
            assert_eq!(
                reply.field("/recommendations"),
                Some(&json!([])),
                "case {case:?}: the required recommendations member must be an empty list, got {}",
                reply.body
            );
        }
    }
}

/// What the session holds is what the call reports, oldest first, bounded by
/// `maxResults` and filtered by `recommendationType`.
#[tokio::test]
async fn the_reported_page_is_the_stored_feed_bounded_and_filtered() {
    let emulator = Emulator::new();
    let (assistant_id, assistant_arn) = emulator.assistant("reporting assistant").await;
    let (session_id, session_arn) = emulator.session(&assistant_id, "reporting session").await;

    // Six entries, cycling through the three published types, so a filtered page has
    // two entries and is not a prefix of the unfiltered one.
    let published: Vec<Published> = (0..6)
        .map(|index| emulator.publish(&assistant_id, &session_id, index))
        .collect();

    // (case, maxResults, recommendationType, how many entries the page holds)
    let rows: Vec<(&str, Option<i64>, Option<&str>, usize)> = vec![
        ("no page size: the modelled maximum", None, None, 6),
        ("one entry", Some(1), None, 1),
        ("three entries", Some(3), None, 3),
        ("exactly the feed", Some(6), None, 6),
        ("more than the feed holds", Some(100), None, 6),
        (
            "filtered to one type",
            None,
            Some(RECOMMENDATION_TYPES[0]),
            2,
        ),
        (
            "filtered to another type",
            None,
            Some(RECOMMENDATION_TYPES[2]),
            2,
        ),
        (
            "filtered and capped",
            Some(1),
            Some(RECOMMENDATION_TYPES[1]),
            1,
        ),
        (
            "filtered to a type nothing published",
            None,
            Some(UNPUBLISHED_TYPE),
            0,
        ),
    ];

    for (case, max_results, recommendation_type, expected_len) in rows {
        let mut query = short_poll();
        if let Some(max_results) = max_results {
            query.push(("maxResults", max_results.to_string()));
        }
        if let Some(recommendation_type) = recommendation_type {
            query.push(("recommendationType", recommendation_type.to_string()));
        }
        let expected: Vec<&Published> = published
            .iter()
            .filter(|row| recommendation_type.is_none_or(|wanted| row.recommendation_type == wanted))
            .take(expected_len)
            .collect();
        assert_eq!(
            expected.len(),
            expected_len,
            "case {case:?}: the table row expects {expected_len} entries, the feed offers {}",
            expected.len()
        );
        let reply = emulator
            .get_recommendations(&assistant_id, &session_id, &query)
            .await;
        assert_reports(case, &reply, &expected);
    }

    // The ARN forms of both labels name the same session, so they report the same feed.
    let by_arn = emulator
        .get_recommendations(&assistant_arn, &session_arn, &short_poll())
        .await;
    assert_reports(
        "both labels in ARN form",
        &by_arn,
        &published.iter().collect::<Vec<&Published>>(),
    );
}

/// `GetRecommendations` is `@readonly`: a read does not consume the feed, so repeating
/// it — at any page size, in any order — answers alike.
#[tokio::test]
async fn reading_the_feed_does_not_consume_it() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("non-consuming assistant").await;
    let (session_id, _) = emulator
        .session(&assistant_id, "non-consuming session")
        .await;
    let published: Vec<Published> = (0..3)
        .map(|index| emulator.publish(&assistant_id, &session_id, index))
        .collect();
    let all: Vec<&Published> = published.iter().collect();

    let first = emulator
        .get_recommendations(&assistant_id, &session_id, &short_poll())
        .await;
    assert_reports("first read", &first, &all);

    // A narrower page in between must not take the entries it reported away.
    let mut narrow_query = short_poll();
    narrow_query.push(("maxResults", "1".to_string()));
    let narrow = emulator
        .get_recommendations(&assistant_id, &session_id, &narrow_query)
        .await;
    assert_reports("page of one", &narrow, &all[..1]);

    let second = emulator
        .get_recommendations(&assistant_id, &session_id, &short_poll())
        .await;
    assert_reports("read after the narrow page", &second, &all);
    assert_eq!(
        second.body, first.body,
        "case \"repeated read\": an unchanged feed must answer alike, got {} then {}",
        first.body, second.body
    );
}

/// A session the named assistant does not hold has nothing available: the call answers
/// the successful empty list the operation documents, not a refusal. Only a label this
/// account holds no ASSISTANT under is `ResourceNotFoundException` (HTTP 404), and the
/// wait the caller asked for does not change either answer.
#[tokio::test]
async fn an_unheld_session_has_nothing_available_and_an_absent_assistant_is_not_found() {
    let emulator = Emulator::new();
    let (assistant_id, assistant_arn) = emulator.assistant("holding assistant").await;
    let (session_id, session_arn) = emulator.session(&assistant_id, "held session").await;
    let (other_id, _) = emulator.assistant("other assistant").await;
    let (other_session_id, other_session_arn) = emulator
        .session(&other_id, "another assistant's session")
        .await;

    // (case, assistant label, session label): a live assistant, and a session it does
    // not hold in every form — absent id, absent ARN, and another assistant's session.
    let nothing_available: Vec<(&str, String, String)> = vec![
        (
            "absent session id",
            assistant_id.clone(),
            ABSENT_UUID.to_string(),
        ),
        (
            "absent session ARN",
            assistant_id.clone(),
            format!(
                "arn:aws:wisdom:{REGION}:{}:session/{assistant_id}/{ABSENT_UUID}",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        (
            "another assistant's session, by id",
            assistant_id.clone(),
            other_session_id,
        ),
        (
            "another assistant's session, by ARN",
            assistant_arn.clone(),
            other_session_arn,
        ),
    ];

    for (case, assistant_label, session_label) in nothing_available {
        let reply = emulator
            .get_recommendations(&assistant_label, &session_label, &short_poll())
            .await;
        assert_reports(case, &reply, &[]);
        assert_eq!(
            reply.field("/recommendations"),
            Some(&json!([])),
            "case {case:?}: expected an empty recommendations list, got {}",
            reply.body
        );
    }

    // The held session still reports its own feed, so "nothing available" is about the
    // session the route names and not about the operation.
    let published = emulator.publish(&assistant_id, &session_id, 0);
    let held = emulator
        .get_recommendations(&assistant_id, &session_arn, &short_poll())
        .await;
    assert_reports("the held session", &held, &[&published]);

    // (case, assistant label): a label this account holds no assistant under.
    let not_found: Vec<(&str, String)> = vec![
        ("absent assistant id", ABSENT_UUID.to_string()),
        (
            "absent assistant ARN",
            format!(
                "arn:aws:wisdom:{REGION}:{}:assistant/{ABSENT_UUID}",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
    ];

    for (case, assistant_label) in not_found {
        // A non-zero wait too: an assistant that does not resolve is refused at once,
        // never waited on.
        for wait in ["0", "3"] {
            let started = Instant::now();
            let reply = emulator
                .get_recommendations(
                    &assistant_label,
                    &session_id,
                    &[("waitTimeSeconds", wait.to_string())],
                )
                .await;
            assert_eq!(
                reply.status, 404,
                "case {case:?} (waitTimeSeconds {wait}): expected HTTP 404, got {} with {}",
                reply.status, reply.body
            );
            assert!(
                reply.error_type.contains("ResourceNotFoundException"),
                "case {case:?}: unexpected error type {:?} with {}",
                reply.error_type,
                reply.body
            );
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "case {case:?}: a refusal must not wait, took {:?}",
                started.elapsed()
            );
        }
    }

    // Deleting the assistant takes its sessions — and their feeds — with it, so the
    // route's own resource is gone and the call is not found.
    let deleted = emulator
        .send("DELETE", &format!("/assistants/{assistant_id}"), None)
        .await;
    assert!(
        (200..300).contains(&deleted.status),
        "case \"delete the assistant\": expected a 2xx, got {} with {}",
        deleted.status,
        deleted.body
    );
    let gone = emulator
        .get_recommendations(&assistant_id, &session_id, &short_poll())
        .await;
    assert_eq!(
        gone.status, 404,
        "case \"session of a deleted assistant\": expected HTTP 404, got {} with {}",
        gone.status, gone.body
    );
}

/// An input outside a modelled constraint is refused as a `ValidationException` (HTTP
/// 400) before anything is reported, while every value inside the modelled range is
/// served: `maxResults` is `range 1..100`, `waitTimeSeconds` is `range 0..20`, and a
/// `nextChunkToken` this service never issued is not a continuation it can serve.
#[tokio::test]
async fn an_input_outside_a_modelled_constraint_is_refused() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("constraint assistant").await;
    let (session_id, _) = emulator.session(&assistant_id, "constraint session").await;
    // A recommendation is available, so the accepted rows — including the 20-second
    // upper bound — answer at once instead of waiting.
    let published = emulator.publish(&assistant_id, &session_id, 0);
    let expected = vec![&published];

    // (case, query, expected status)
    let rows: Vec<(&str, Query, u16)> = vec![
        (
            "maxResults at the lower bound",
            vec![("waitTimeSeconds", "0".to_string()), ("maxResults", "1".to_string())],
            200,
        ),
        (
            "maxResults at the upper bound",
            vec![("waitTimeSeconds", "0".to_string()), ("maxResults", "100".to_string())],
            200,
        ),
        (
            "maxResults one below the lower bound",
            vec![("maxResults", "0".to_string())],
            400,
        ),
        (
            "maxResults one past the upper bound",
            vec![("maxResults", "101".to_string())],
            400,
        ),
        (
            "waitTimeSeconds at the lower bound",
            vec![("waitTimeSeconds", "0".to_string())],
            200,
        ),
        (
            "waitTimeSeconds at the upper bound",
            vec![("waitTimeSeconds", "20".to_string())],
            200,
        ),
        (
            "waitTimeSeconds below the lower bound",
            vec![("waitTimeSeconds", "-1".to_string())],
            400,
        ),
        (
            "waitTimeSeconds past the upper bound",
            vec![("waitTimeSeconds", "21".to_string())],
            400,
        ),
        (
            "a nextChunkToken this service never issued",
            vec![("nextChunkToken", "not-a-chunk-token".to_string())],
            400,
        ),
        (
            "a nextChunkToken at the modelled upper length",
            vec![("nextChunkToken", "t".repeat(2048))],
            400,
        ),
        (
            "a nextChunkToken past the modelled upper length",
            vec![("nextChunkToken", "t".repeat(2049))],
            400,
        ),
    ];

    for (case, query, expected_status) in rows {
        let started = Instant::now();
        let reply = emulator
            .get_recommendations(&assistant_id, &session_id, &query)
            .await;
        assert_eq!(
            reply.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} with {}",
            reply.status, reply.body
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "case {case:?}: an available recommendation must answer sooner than the wait, took {:?}",
            started.elapsed()
        );
        if expected_status == 400 {
            assert!(
                reply.error_type.contains("ValidationException"),
                "case {case:?}: unexpected error type {:?} with {}",
                reply.error_type,
                reply.body
            );
        } else {
            assert_reports(case, &reply, &expected);
        }
    }
}

/// `waitTimeSeconds` is the wait the CALLER asked for: a short poll answers at once, a
/// longer wait returns as soon as a recommendation is made available, and a wait nothing
/// arrives in expires with the empty list the contract describes.
#[tokio::test]
async fn a_wait_returns_as_soon_as_a_recommendation_is_available() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("polling assistant").await;
    let (session_id, _) = emulator.session(&assistant_id, "polling session").await;

    // A short poll (the modelled default) only reports what is already available, so it
    // answers at once.
    let started = Instant::now();
    let short = emulator
        .get_recommendations(&assistant_id, &session_id, &short_poll())
        .await;
    let short_poll_took = started.elapsed();
    assert_reports("short poll on an empty feed", &short, &[]);
    assert!(
        short_poll_took < Duration::from_secs(1),
        "case \"short poll on an empty feed\": expected an immediate answer, took {short_poll_took:?}"
    );

    // A wait nothing arrives in expires with a successful empty list, and the call
    // really waited the second the caller asked for.
    let started = Instant::now();
    let expired = emulator
        .get_recommendations(
            &assistant_id,
            &session_id,
            &[("waitTimeSeconds", "1".to_string())],
        )
        .await;
    let waited = started.elapsed();
    assert_reports("a wait that expires", &expired, &[]);
    assert!(
        waited >= Duration::from_secs(1),
        "case \"a wait that expires\": expected the call to wait the requested second, took {waited:?}"
    );

    // A recommendation made available while the call waits is reported as soon as it is
    // there — sooner than the wait.
    let publisher = {
        let engine = Arc::clone(&emulator.engine);
        let assistant = assistant_id.clone();
        let session = session_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            publish(&engine, &assistant, &session, 2)
        })
    };
    let started = Instant::now();
    let awaited = emulator
        .get_recommendations(
            &assistant_id,
            &session_id,
            &[("waitTimeSeconds", "20".to_string())],
        )
        .await;
    let waited = started.elapsed();
    let published = publisher.await.expect("the publishing task");
    assert_reports(
        "a recommendation arriving mid-wait",
        &awaited,
        &[&published],
    );
    assert!(
        waited < Duration::from_secs(10),
        "case \"a recommendation arriving mid-wait\": expected the call to return as soon as the \
         recommendation was available, took {waited:?}"
    );
}

/// The feed is bounded: publishing past `MAX_PENDING_RECOMMENDATIONS_PER_SESSION`
/// forgets the OLDEST recommendation (and the trigger that named only it) rather than
/// growing the store without limit, so a full feed still fills the largest page the
/// model allows.
#[tokio::test]
async fn the_feed_is_bounded_and_forgets_the_oldest_entry() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("bounded assistant").await;
    let (session_id, _) = emulator.session(&assistant_id, "bounded session").await;

    let ceiling = state::MAX_PENDING_RECOMMENDATIONS_PER_SESSION;
    let overshoot = 3;
    let published: Vec<Published> = (0..ceiling + overshoot)
        .map(|index| emulator.publish(&assistant_id, &session_id, index))
        .collect();

    let mut query = short_poll();
    query.push(("maxResults", ceiling.to_string()));
    let reply = emulator
        .get_recommendations(&assistant_id, &session_id, &query)
        .await;
    assert_reports(
        "a feed filled past its ceiling",
        &reply,
        &published[overshoot..].iter().collect::<Vec<&Published>>(),
    );
}
