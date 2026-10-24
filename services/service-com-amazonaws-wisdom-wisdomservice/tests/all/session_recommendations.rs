// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetRecommendations`, driven over HTTP through the same
//! router the emulator serves with.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one
//! recorded request:
//!
//! * `GetRecommendations` REPORTS the recommendations persisted on the session
//!   the route names — every recommendation it answers with, and every trigger,
//!   is the stored record, oldest first — for every label form the model's
//!   `UuidOrArn` accepts on either label;
//! * a session with nothing available answers HTTP 200 with an EMPTY
//!   `recommendations` list, which is what the modelled contract says a wait
//!   that expires with no recommendation returns;
//! * `maxResults` bounds the page, for every page size the model allows;
//! * the operation is `@readonly`: reading does not consume the feed, so
//!   repeating a read answers alike;
//! * a label this account holds no ASSISTANT under is `ResourceNotFoundException`
//!   (HTTP 404), while a session the named assistant does not hold has nothing
//!   available and answers the same successful empty list (the recorded unit
//!   `wisdom.get-recommendations-nonexistent-session-rejected` shows the endpoint
//!   answering exactly that);
//! * `waitTimeSeconds` is the wait the caller asked for: a short poll (0)
//!   answers at once, a longer wait returns as soon as a recommendation is
//!   available and otherwise waits out its wait and answers empty;
//! * the feed is bounded, so publishing past the ceiling forgets the OLDEST
//!   recommendation rather than growing the store without limit.
//!
//! The recommendations are put on the session's feed directly, through the
//! state seam `RecommendationFeed::publish`: real Wisdom makes a recommendation
//! available out of band (from the contact's detected intent), and no operation
//! of THIS model publishes one — `GetRecommendations` only reports what the
//! session holds. Everything else goes over the wire, so a response that is not
//! computed from persisted state cannot pass.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_wisdom_wisdomservice::ops::types::{
    ContentReference, Document, DocumentText, QueryRecommendationTriggerData, RecommendationData,
    RecommendationTrigger, RecommendationTriggerData,
};
use service_com_amazonaws_wisdom_wisdomservice::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for; the handlers read it
/// out of the SigV4 credential scope to build ARNs.
const REGION: &str = "us-west-2";

/// The routing key this service answers under (`src/service.rs`'s
/// `OWN_REGISTRATION_KEY`): the shared `wisdom` endpoint identity is served by
/// the Amazon Q in Connect crate, so this legacy model is addressed by its own
/// key.
const ROUTE_KEY: &str = "com-amazonaws-wisdom-wisdomservice";

/// A UUID no resource of this suite is created under, for the not-found
/// assertions.
const ABSENT_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// The only `RecommendationType` the model declares
/// (`smithy_query(query="shape",shape="RecommendationType")`), written out
/// rather than imported from the implementation so the assertions pin the
/// modelled value.
const TYPE_KNOWLEDGE_CONTENT: &str = "KNOWLEDGE_CONTENT";

/// Every `RelevanceLevel` the model declares, in model order
/// (`smithy_query(query="shape",shape="RelevanceLevel")`).
const RELEVANCE_LEVELS: [&str; 3] = ["HIGH", "MEDIUM", "LOW"];

/// Every `RecommendationSourceType` the model declares
/// (`smithy_query(query="shape",shape="RecommendationSourceType")`).
const SOURCE_TYPES: [&str; 3] = ["ISSUE_DETECTION", "RULE_EVALUATION", "OTHER"];

/// The only `RecommendationTriggerType` the model declares
/// (`smithy_query(query="shape",shape="RecommendationTriggerType")`).
const TRIGGER_TYPE_QUERY: &str = "QUERY";

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

    /// The reported `recommendations` list (empty when the member is absent,
    /// which the required member never is).
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

/// Percent-encode one path label the way an AWS SDK encodes a non-greedy
/// `@httpLabel`: an ARN label carries `:` and `/`, which must not be read as
/// path structure. A bare uuid label passes through unchanged.
fn label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// What one published recommendation is made of, so the assertions compare the
/// wire answer against the STORED record rather than against a literal.
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
    source: &'static str,
}

/// An emulator with just this service mounted on a fresh engine, driven
/// in-process through the production router. The engine is kept so the suite can
/// put recommendations on a session's feed.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes
        // by the SigV4 credential scope below, which is also where an ARN's
        // region comes from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{ROUTE_KEY}/aws4_request, \
             SignedHeaders=host, Signature=not-verified-in-this-suite"
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
        let bytes = response.body().clone();
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
                &format!("/assistants/{}/sessions", label(assistant_label)),
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

    /// `GetRecommendations` on one (assistant label, session label) pair, with
    /// the query members the row names.
    async fn get_recommendations(
        &self,
        assistant_label: &str,
        session_label: &str,
        max_results: Option<i64>,
        wait_time_seconds: Option<i64>,
    ) -> Reply {
        let mut query: Vec<String> = Vec::new();
        if let Some(max_results) = max_results {
            query.push(format!("maxResults={max_results}"));
        }
        if let Some(wait_time_seconds) = wait_time_seconds {
            query.push(format!("waitTimeSeconds={wait_time_seconds}"));
        }
        let suffix = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        self.send(
            "GET",
            &format!(
                "/assistants/{}/sessions/{}/recommendations{suffix}",
                label(assistant_label),
                label(session_label)
            ),
            None,
        )
        .await
    }

    /// Make one recommendation available on a session's feed, the way the real
    /// service makes one available out of band, and hand back the stored record
    /// the assertions compare the wire answer against.
    fn publish(&self, assistant_label: &str, session_label: &str, index: usize) -> Published {
        publish(&self.engine, assistant_label, session_label, index)
    }
}

/// Make one recommendation available on a session's feed, through the state
/// seam a producer of recommendations writes to.
///
/// Takes the engine rather than the whole emulator, so a task that only
/// publishes (the mid-wait publisher below) needs nothing else.
fn publish(
    engine: &engine::InMemoryEngine,
    assistant_label: &str,
    session_label: &str,
    index: usize,
) -> Published {
    let published = Published {
        recommendation_id: state::new_resource_id(),
        trigger_id: state::new_resource_id(),
        excerpt: format!("excerpt of recommendation {index}"),
        title: format!("title {index}"),
        query_text: format!("what the contact asked, take {index}"),
        knowledge_base_id: state::new_resource_id(),
        content_id: state::new_resource_id(),
        // A score that differs per row, so a reported score cannot be a
        // constant that happens to match.
        relevance_score: 0.125 * (index as f64 + 1.0),
        relevance_level: RELEVANCE_LEVELS[index % RELEVANCE_LEVELS.len()],
        source: SOURCE_TYPES[index % SOURCE_TYPES.len()],
    };
    let store = state::service_state(engine).expect("this service's store");
    let mut store = store.lock();
    let session = store
        .session_mut(assistant_label, session_label)
        .expect("a session to publish onto");
    session.recommendations.publish(
        RecommendationData {
            recommendation_id: published.recommendation_id.clone(),
            document: Document {
                content_reference: ContentReference {
                    knowledge_base_arn: None,
                    knowledge_base_id: Some(published.knowledge_base_id.clone()),
                    content_arn: None,
                    content_id: Some(published.content_id.clone()),
                },
                title: Some(DocumentText {
                    text: Some(published.title.clone()),
                    highlights: None,
                }),
                excerpt: Some(DocumentText {
                    text: Some(published.excerpt.clone()),
                    highlights: None,
                }),
            },
            relevance_score: published.relevance_score,
            relevance_level: Some(published.relevance_level.to_string()),
            r#type: Some(TYPE_KNOWLEDGE_CONTENT.to_string()),
        },
        Some(RecommendationTrigger {
            id: published.trigger_id.clone(),
            r#type: TRIGGER_TYPE_QUERY.to_string(),
            source: published.source.to_string(),
            data: RecommendationTriggerData::Query(QueryRecommendationTriggerData {
                text: Some(published.query_text.clone()),
            }),
            recommendation_ids: vec![published.recommendation_id.clone()],
        }),
    );
    published
}

/// Every reported recommendation is the stored record, in the order the service
/// made them available, and the reported triggers are the triggers of the
/// recommendations IN the page.
fn assert_reports(case: &str, reply: &Reply, expected: &[Published]) {
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
            Some(TYPE_KNOWLEDGE_CONTENT),
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
            Some(TRIGGER_TYPE_QUERY),
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

/// A session with nothing available answers HTTP 200 with an empty
/// `recommendations` list — for every label form the model accepts and every
/// page size it allows, including both published `maxResults` bounds.
#[tokio::test]
async fn a_session_with_nothing_available_answers_an_empty_list() {
    let emulator = Emulator::new();
    let (assistant_id, assistant_arn) = emulator.assistant("empty-feed assistant").await;
    let (session_id, session_arn) = emulator.session(&assistant_id, "empty-feed session").await;

    // (case, assistant label, session label): both `UuidOrArn` forms on both
    // labels must resolve to the same session.
    let labels: Vec<(&str, &str, &str)> = vec![
        ("id and id", assistant_id.as_str(), session_id.as_str()),
        ("arn and id", assistant_arn.as_str(), session_id.as_str()),
        ("id and arn", assistant_id.as_str(), session_arn.as_str()),
        ("arn and arn", assistant_arn.as_str(), session_arn.as_str()),
    ];
    // `maxResults` is modelled `range 1..100`: no page size, both bounds and one
    // size inside.
    let page_sizes: [Option<i64>; 4] = [None, Some(1), Some(7), Some(100)];

    for (labels_case, assistant_label, session_label) in labels {
        for max_results in page_sizes {
            let case = format!("{labels_case}, maxResults {max_results:?}");
            let reply = emulator
                .get_recommendations(assistant_label, session_label, max_results, Some(0))
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
/// `maxResults` — over every page size the model allows for a four-entry feed.
#[tokio::test]
async fn the_reported_page_is_the_stored_feed_bounded_by_max_results() {
    let emulator = Emulator::new();
    let (assistant_id, assistant_arn) = emulator.assistant("reporting assistant").await;
    let (session_id, session_arn) = emulator.session(&assistant_id, "reporting session").await;

    let published: Vec<Published> = (0..4)
        .map(|index| emulator.publish(&assistant_id, &session_id, index))
        .collect();

    // (case, maxResults, how many entries the page holds)
    let rows: Vec<(&str, Option<i64>, usize)> = vec![
        ("no page size: the modelled maximum", None, 4),
        ("one entry", Some(1), 1),
        ("two entries", Some(2), 2),
        ("exactly the feed", Some(4), 4),
        ("more than the feed holds", Some(100), 4),
    ];

    for (case, max_results, expected_len) in rows {
        let reply = emulator
            .get_recommendations(&assistant_id, &session_id, max_results, Some(0))
            .await;
        assert_reports(case, &reply, &published[..expected_len]);
    }

    // The ARN forms of both labels name the same session, so they report the
    // same feed.
    let by_arn = emulator
        .get_recommendations(&assistant_arn, &session_arn, None, Some(0))
        .await;
    assert_reports("both labels in ARN form", &by_arn, &published);
}

/// `GetRecommendations` is `@readonly`: a read does not consume the feed, so
/// repeating it — at any page size, in any order — answers alike.
#[tokio::test]
async fn reading_the_feed_does_not_consume_it() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("non-consuming assistant").await;
    let (session_id, _) = emulator.session(&assistant_id, "non-consuming session").await;
    let published: Vec<Published> = (0..3)
        .map(|index| emulator.publish(&assistant_id, &session_id, index))
        .collect();

    let first = emulator
        .get_recommendations(&assistant_id, &session_id, None, Some(0))
        .await;
    assert_reports("first read", &first, &published);

    // A narrower page in between must not take the entries it reported away.
    let narrow = emulator
        .get_recommendations(&assistant_id, &session_id, Some(1), Some(0))
        .await;
    assert_reports("page of one", &narrow, &published[..1]);

    let second = emulator
        .get_recommendations(&assistant_id, &session_id, None, Some(0))
        .await;
    assert_reports("read after the narrow page", &second, &published);
    assert_eq!(
        second.body, first.body,
        "case \"repeated read\": an unchanged feed must answer alike, got {} then {}",
        first.body, second.body
    );
}

/// A session the named assistant does not hold has nothing available: the call
/// answers the successful empty list the operation documents, not a refusal. Only
/// a label this account holds no ASSISTANT under is `ResourceNotFoundException`
/// (HTTP 404), and the wait the caller asked for does not change either answer.
#[tokio::test]
async fn an_unheld_session_has_nothing_available_and_an_absent_assistant_is_not_found() {
    let emulator = Emulator::new();
    let (assistant_id, assistant_arn) = emulator.assistant("holding assistant").await;
    let (session_id, session_arn) = emulator.session(&assistant_id, "held session").await;
    let (other_id, _) = emulator.assistant("other assistant").await;
    let (other_session_id, other_session_arn) =
        emulator.session(&other_id, "another assistant's session").await;
    let absent_session_arn = format!(
        "arn:aws:wisdom:{REGION}:{}:session/{assistant_id}/{ABSENT_UUID}",
        engine::emulator_account_id()
    );

    // (case, assistant label, session label): a live assistant, and a session it
    // does not hold in every form — absent id, absent ARN, another assistant's.
    let nothing_available: Vec<(&str, String, String)> = vec![
        (
            "absent session id",
            assistant_id.clone(),
            ABSENT_UUID.to_string(),
        ),
        (
            "absent session ARN",
            assistant_id.clone(),
            absent_session_arn,
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
            .get_recommendations(&assistant_label, &session_label, None, Some(0))
            .await;
        assert_reports(case, &reply, &[]);
        assert_eq!(
            reply.field("/recommendations"),
            Some(&json!([])),
            "case {case:?}: expected an empty recommendations list, got {}",
            reply.body
        );
    }

    // The held session still reports its own feed, so "nothing available" is about
    // the session the route names and not about the operation.
    let published = vec![emulator.publish(&assistant_id, &session_id, 0)];
    let held = emulator
        .get_recommendations(&assistant_id, &session_arn, None, Some(0))
        .await;
    assert_reports("the held session", &held, &published);

    // (case, assistant label): a label this account holds no assistant under.
    let not_found: Vec<(&str, String)> = vec![
        ("absent assistant id", ABSENT_UUID.to_string()),
        (
            "absent assistant ARN",
            format!(
                "arn:aws:wisdom:{REGION}:{}:assistant/{ABSENT_UUID}",
                engine::emulator_account_id()
            ),
        ),
    ];

    for (case, assistant_label) in not_found {
        // A non-zero wait too: an assistant that does not resolve is refused at
        // once, never waited on.
        for wait in [Some(0), Some(3)] {
            let started = Instant::now();
            let reply = emulator
                .get_recommendations(&assistant_label, &session_id, None, wait)
                .await;
            assert_eq!(
                reply.status, 404,
                "case {case:?} (waitTimeSeconds {wait:?}): expected HTTP 404, got {} with {}",
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
    assert_eq!(
        deleted.status, 204,
        "case \"delete the assistant\": expected HTTP 204 (DeleteAssistant models no output), \
         got {} with {}",
        deleted.status, deleted.body
    );
    let gone = emulator
        .get_recommendations(&assistant_id, &session_id, None, Some(0))
        .await;
    assert_eq!(
        gone.status, 404,
        "case \"session of a deleted assistant\": expected HTTP 404, got {} with {}",
        gone.status, gone.body
    );
}

/// An input outside a modelled constraint is refused as a `ValidationException`
/// (HTTP 400) before anything is reported, while every value inside the
/// modelled range is served: `maxResults` is `range 1..100` and
/// `waitTimeSeconds` is `range 0..20`, so both bounds of each hold and one step
/// past either does not.
#[tokio::test]
async fn an_input_outside_a_modelled_constraint_is_refused() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("constraint assistant").await;
    let (session_id, _) = emulator.session(&assistant_id, "constraint session").await;
    // A recommendation is available, so the accepted rows — including the
    // 20-second upper bound — answer at once instead of waiting.
    let published = vec![emulator.publish(&assistant_id, &session_id, 0)];

    // (case, maxResults, waitTimeSeconds, expected status)
    let rows: Vec<(&str, Option<i64>, Option<i64>, u16)> = vec![
        ("maxResults at the lower bound", Some(1), Some(0), 200),
        ("maxResults at the upper bound", Some(100), Some(0), 200),
        ("maxResults one below the lower bound", Some(0), Some(0), 400),
        ("maxResults one past the upper bound", Some(101), Some(0), 400),
        ("waitTimeSeconds at the lower bound", None, Some(0), 200),
        ("waitTimeSeconds at the upper bound", None, Some(20), 200),
        ("waitTimeSeconds below the lower bound", None, Some(-1), 400),
        ("waitTimeSeconds past the upper bound", None, Some(21), 400),
    ];

    for (case, max_results, wait_time_seconds, expected) in rows {
        let started = Instant::now();
        let reply = emulator
            .get_recommendations(&assistant_id, &session_id, max_results, wait_time_seconds)
            .await;
        assert_eq!(
            reply.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with {}",
            reply.status, reply.body
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "case {case:?}: an available recommendation must answer sooner than the wait, took {:?}",
            started.elapsed()
        );
        if expected == 400 {
            assert!(
                reply.error_type.contains("ValidationException"),
                "case {case:?}: unexpected error type {:?} with {}",
                reply.error_type,
                reply.body
            );
        } else {
            assert_reports(case, &reply, &published);
        }
    }
}

/// `waitTimeSeconds` is the wait the CALLER asked for: a short poll answers at
/// once, a longer wait returns as soon as a recommendation is made available,
/// and a wait nothing arrives in expires with the empty list the contract
/// describes.
#[tokio::test]
async fn a_wait_returns_as_soon_as_a_recommendation_is_available() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("polling assistant").await;
    let (session_id, _) = emulator.session(&assistant_id, "polling session").await;

    // A short poll (the modelled default) only reports what is already
    // available, so it answers at once.
    let started = Instant::now();
    let short = emulator
        .get_recommendations(&assistant_id, &session_id, None, Some(0))
        .await;
    let short_poll = started.elapsed();
    assert_reports("short poll on an empty feed", &short, &[]);
    assert!(
        short_poll < Duration::from_secs(1),
        "case \"short poll on an empty feed\": expected an immediate answer, took {short_poll:?}"
    );

    // A wait nothing arrives in expires with a successful empty list, and the
    // call really waited the second the caller asked for.
    let started = Instant::now();
    let expired = emulator
        .get_recommendations(&assistant_id, &session_id, None, Some(1))
        .await;
    let waited = started.elapsed();
    assert_reports("a wait that expires", &expired, &[]);
    assert!(
        waited >= Duration::from_secs(1),
        "case \"a wait that expires\": expected the call to wait the requested second, took {waited:?}"
    );

    // A recommendation made available while the call waits is reported as soon
    // as it is there — sooner than the wait.
    let published = {
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
        .get_recommendations(&assistant_id, &session_id, None, Some(20))
        .await;
    let waited = started.elapsed();
    let published = published.await.expect("the publishing task");
    assert_reports("a recommendation arriving mid-wait", &awaited, &[published]);
    assert!(
        waited < Duration::from_secs(10),
        "case \"a recommendation arriving mid-wait\": expected the call to return as soon as the \
         recommendation was available, took {waited:?}"
    );
}

/// The feed is bounded: publishing past
/// `MAX_PENDING_RECOMMENDATIONS_PER_SESSION` forgets the OLDEST recommendation
/// (and the trigger that named only it) rather than growing the store without
/// limit, so a full feed still fills the largest page the model allows.
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

    let reply = emulator
        .get_recommendations(
            &assistant_id,
            &session_id,
            Some(i64::try_from(ceiling).expect("the ceiling fits a page size")),
            Some(0),
        )
        .await;
    assert_reports(
        "a feed filled past its ceiling",
        &reply,
        &published[overshoot..],
    );
}
