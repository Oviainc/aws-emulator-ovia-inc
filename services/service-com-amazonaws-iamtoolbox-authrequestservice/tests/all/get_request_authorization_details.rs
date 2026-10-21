// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetRequestAuthorizationDetails`, the service's only operation.
//!
//! The rule under test: a read answers from the authorization-details record stored for
//! the requested id — its own request context, its own evaluated policies, and its own
//! evaluation list sliced into pages — for as long as that record is inside the
//! retention window, and answers the modelled error otherwise.
//!
//! Every assertion is driven over a TABLE of records whose evaluation counts straddle
//! the page size, so nothing here can pass against a response built for one recorded
//! shape: the counts, the ids, the context keys, the effects and the policy sets all
//! vary row by row.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use aws_smithy_types::{Document, Number};
use serde_json::{json, Map, Value};

use service_com_amazonaws_iamtoolbox_authrequestservice::ops::types::{
    AttachedTo, Evaluation, MatchedPolicy, MatchedStatement, PolicyInfo,
    EVALUATED_EFFECT_VALUES, POLICY_TYPE_VALUES, STATEMENT_EFFECT_VALUES,
};
use service_com_amazonaws_iamtoolbox_authrequestservice::state::{
    AuthorizationDetailsRecord, AUTHORIZATION_DETAILS_RETENTION,
};
use service_com_amazonaws_iamtoolbox_authrequestservice::{
    ComAmazonawsIamtoolboxAuthrequestserviceState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The page size the handler slices `evaluations` with. Mirrored here so a row can say
/// how many pages it expects; the walk itself follows `nextToken` and never assumes it.
const PAGE_SIZE: usize = 10;

/// The emulator under test: the shared engine (the only store), plus a router with this
/// one service mounted, driven through the same `dispatch` the production front door
/// uses.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// Record a denial in the shared engine state — the seam that stands in for the
    /// denial a real service would have recorded, since the model declares no write.
    fn record(&self, record: AuthorizationDetailsRecord) {
        self.engine
            .state::<ComAmazonawsIamtoolboxAuthrequestserviceState>()
            .lock()
            .record_denial(record);
    }

    /// `GET /authorization-details/{id}[?nextToken=…]`, returning the status and the
    /// raw body bytes.
    async fn get_raw(&self, id: &str, next_token: Option<&str>) -> (u16, bytes::Bytes) {
        let mut uri = format!("http://iam.amazonaws.com/authorization-details/{id}");
        if let Some(token) = next_token {
            uri.push_str("?nextToken=");
            uri.push_str(token);
        }
        let request = http::Request::builder()
            .method("GET")
            .uri(&uri)
            .header("host", "iam.amazonaws.com")
            .body(bytes::Bytes::new())
            .expect("build the GetRequestAuthorizationDetails request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, response.into_body())
    }

    /// The same read, with the body parsed as JSON (restJson1).
    async fn get(&self, id: &str, next_token: Option<&str>) -> (u16, Value) {
        let (status, body) = self.get_raw(id, next_token).await;
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or_else(|e| {
                panic!(
                    "case id={id:?} token={next_token:?}: response body must be JSON, \
                     got {:?} ({e})",
                    String::from_utf8_lossy(&body)
                )
            })
        };
        (status, parsed)
    }
}

// ---------------------------------------------------------------------------
// Record fixtures: one generator for the stored value, one for the JSON the wire
// must carry for it. They are written out separately on purpose — the expected
// JSON is what the model says the member names and shapes are, not a re-run of
// the code under test.
// ---------------------------------------------------------------------------

/// The request-level context of the denial recorded for `id`: a map<string,document>
/// whose values cover the document kinds a context key can hold.
fn request_context(id: &str) -> HashMap<String, Document> {
    HashMap::from([
        (
            "aws:PrincipalArn".to_string(),
            Document::String(engine::emulator_arn("iam", "", &format!("user/{id}"))),
        ),
        (
            "aws:MultiFactorAuthPresent".to_string(),
            Document::Bool(id.len().is_multiple_of(2)),
        ),
        (
            "aws:TokenIssueTime".to_string(),
            Document::String(format!("2026-10-21T21:0{}:00Z", id.len() % 6)),
        ),
        (
            "aws:MultiFactorAuthAge".to_string(),
            Document::Number(Number::PosInt(60 * id.len() as u64)),
        ),
    ])
}

/// The JSON `request_context` must serialize to.
fn request_context_json(id: &str) -> Value {
    json!({
        "aws:PrincipalArn": engine::emulator_arn("iam", "", &format!("user/{id}")),
        "aws:MultiFactorAuthPresent": id.len().is_multiple_of(2),
        "aws:TokenIssueTime": format!("2026-10-21T21:0{}:00Z", id.len() % 6),
        "aws:MultiFactorAuthAge": 60 * id.len() as u64,
    })
}

/// Evaluation number `index` of the denial recorded for `id`. Distinct for every
/// (id, index) pair, so a page can be placed exactly.
///
/// The effect values are the model's own `EvaluatedEffect` / `StatementEffect` enum
/// members, taken from the generated catalogs in `ops::types::shared_types` rather than
/// written out here, so one record carries mixed ALLOW / EXPLICIT_DENY / IMPLICIT_DENY.
fn evaluation(id: &str, index: usize) -> Evaluation {
    Evaluation {
        action: format!("iamtoolbox:Action{index}"),
        resource: engine::emulator_arn("s3", "", &format!("{id}-bucket/object-{index}")),
        context: Some(HashMap::from([(
            "aws:RequestedRegion".to_string(),
            Document::String(format!("us-west-{}", index % 3)),
        )])),
        evaluated_effect: Some(
            EVALUATED_EFFECT_VALUES[index % EVALUATED_EFFECT_VALUES.len()].to_string(),
        ),
        matched_policies: Some(vec![MatchedPolicy {
            uri: format!("policy://{id}/evaluation-{index}"),
            matched_statements: Some(vec![MatchedStatement {
                // Half the statements carry no sid, so an omitted optional member is
                // exercised too.
                sid: index.is_multiple_of(2).then(|| format!("Statement{index}")),
                evaluated_effect: Some(
                    STATEMENT_EFFECT_VALUES[index % STATEMENT_EFFECT_VALUES.len()].to_string(),
                ),
            }]),
        }]),
    }
}

/// The JSON [`evaluation`] must serialize to, with the model's wire member names.
fn evaluation_json(id: &str, index: usize) -> Value {
    let mut statement = Map::new();
    if index.is_multiple_of(2) {
        statement.insert("sid".to_string(), json!(format!("Statement{index}")));
    }
    statement.insert(
        "evaluatedEffect".to_string(),
        json!(STATEMENT_EFFECT_VALUES[index % STATEMENT_EFFECT_VALUES.len()]),
    );
    json!({
        "action": format!("iamtoolbox:Action{index}"),
        "resource": engine::emulator_arn("s3", "", &format!("{id}-bucket/object-{index}")),
        "context": { "aws:RequestedRegion": format!("us-west-{}", index % 3) },
        "evaluatedEffect": EVALUATED_EFFECT_VALUES[index % EVALUATED_EFFECT_VALUES.len()],
        "matchedPolicies": [{
            "uri": format!("policy://{id}/evaluation-{index}"),
            "matchedStatements": [Value::Object(statement)],
        }],
    })
}

/// The policies evaluated for the denial recorded for `id`. The `type` values come from
/// the model's `PolicyType` catalog; which two a record carries varies with the id, so
/// two records do not share a policy list.
fn policies(id: &str) -> Vec<PolicyInfo> {
    let first = id.len() % POLICY_TYPE_VALUES.len();
    let second = (first + 1) % POLICY_TYPE_VALUES.len();
    vec![
        PolicyInfo {
            r#type: Some(POLICY_TYPE_VALUES[first].to_string()),
            inline: Some(false),
            uri: Some(format!("policy://{id}/attached")),
            attached_to: Some(vec![AttachedTo {
                arn: Some(engine::emulator_arn("iam", "", &format!("user/{id}"))),
            }]),
        },
        PolicyInfo {
            r#type: Some(POLICY_TYPE_VALUES[second].to_string()),
            inline: Some(true),
            uri: None,
            attached_to: None,
        },
    ]
}

/// The JSON [`policies`] must serialize to.
fn policies_json(id: &str) -> Value {
    let first = id.len() % POLICY_TYPE_VALUES.len();
    let second = (first + 1) % POLICY_TYPE_VALUES.len();
    json!([
        {
            "type": POLICY_TYPE_VALUES[first],
            "inline": false,
            "uri": format!("policy://{id}/attached"),
            "attachedTo": [{ "arn": engine::emulator_arn("iam", "", &format!("user/{id}")) }],
        },
        {
            "type": POLICY_TYPE_VALUES[second],
            "inline": true,
        },
    ])
}

/// A record for `id` carrying `evaluation_count` evaluations.
fn record(id: &str, evaluation_count: usize) -> AuthorizationDetailsRecord {
    AuthorizationDetailsRecord::new(
        id,
        request_context(id),
        (0..evaluation_count).map(|i| evaluation(id, i)).collect(),
        policies(id),
    )
}

/// The error code (`__type`) and message of a modelled error response.
fn error_of(body: &Value, case: &str) -> (String, String) {
    let code = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: error body must carry __type, got {body}"))
        // `__type` may be namespaced (`com.amazonaws.iamtoolbox#Name`).
        .rsplit('#')
        .next()
        .expect("a non-empty __type")
        .to_string();
    let message = body
        .get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: error body must carry a message, got {body}"))
        .to_string();
    (code, message)
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

/// Walking the pages of a stored record yields exactly its evaluations, in order, with
/// no duplicates and no gaps — for evaluation counts on both sides of the page size —
/// and every page repeats the request-level members.
#[tokio::test]
async fn walking_pages_yields_exactly_the_stored_evaluations_in_order() {
    // Rows differ in the one thing the pagination rule generalises over: how many
    // evaluations the record holds, relative to the page size.
    let cases: &[(&str, usize)] = &[
        ("emptyrecord", 0),
        ("single", 1),
        ("justunder", PAGE_SIZE - 1),
        ("exactpage", PAGE_SIZE),
        ("onepast", PAGE_SIZE + 1),
        ("manypages", PAGE_SIZE * 2 + 5),
        ("abc123def", 3),
    ];

    let emulator = Emulator::new();
    for (id, count) in cases {
        emulator.record(record(id, *count));
    }

    for (id, count) in cases {
        let case = format!("id={id} evaluations={count}");
        let mut seen: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;

        loop {
            let (status, body) = emulator.get(id, token.as_deref()).await;
            pages += 1;
            assert_eq!(status, 200, "case {case}: page {pages} must be 200, got {body}");

            assert_eq!(
                body.get("requestContext"),
                Some(&request_context_json(id)),
                "case {case}: page {pages} must repeat the stored requestContext, got {body}"
            );
            assert_eq!(
                body.get("policies"),
                Some(&policies_json(id)),
                "case {case}: page {pages} must repeat the stored policies, got {body}"
            );

            let page = body
                .get("evaluations")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case}: page {pages} must carry evaluations, got {body}"));
            let remaining = count - seen.len();
            assert_eq!(
                page.len(),
                remaining.min(PAGE_SIZE),
                "case {case}: page {pages} must carry min(page size, remaining) evaluations, \
                 got {}",
                page.len()
            );
            seen.extend(page.iter().cloned());

            match body.get("nextToken").and_then(Value::as_str) {
                Some(next) => {
                    assert!(
                        seen.len() < *count,
                        "case {case}: page {pages} must omit nextToken once every \
                         evaluation has been returned, got {next:?}"
                    );
                    token = Some(next.to_string());
                }
                None => {
                    assert_eq!(
                        seen.len(),
                        *count,
                        "case {case}: the last page must complete the stored evaluations, \
                         got {} of {count}",
                        seen.len()
                    );
                    break;
                }
            }
            assert!(pages <= count + 2, "case {case}: pagination must terminate");
        }

        let expected: Vec<Value> = (0..*count).map(|i| evaluation_json(id, i)).collect();
        assert_eq!(
            seen, expected,
            "case {case}: the concatenated pages must equal the stored evaluations in order"
        );
        assert_eq!(
            pages,
            if *count == 0 { 1 } else { count.div_ceil(PAGE_SIZE) },
            "case {case}: the walk must take ceil(count / page size) pages"
        );
    }
}

/// The read is `@readonly`: repeating it answers a byte-identical body, leaves the
/// record in the store, and a token stays resolvable however many times it is used.
#[tokio::test]
async fn repeated_reads_answer_identically_and_consume_nothing() {
    let cases: &[(&str, usize)] = &[("idempotent", 0), ("twopages", PAGE_SIZE + 2)];
    let emulator = Emulator::new();
    for (id, count) in cases {
        emulator.record(record(id, *count));
    }

    for (id, count) in cases {
        let case = format!("id={id} evaluations={count}");
        let (first_status, first) = emulator.get_raw(id, None).await;
        let (second_status, second) = emulator.get_raw(id, None).await;
        assert_eq!(
            (first_status, &first),
            (second_status, &second),
            "case {case}: two identical reads must answer identically"
        );

        // A token may be followed twice and resolves to the same page both times.
        if let Some(token) = serde_json::from_slice::<Value>(&first)
            .ok()
            .and_then(|body| body.get("nextToken").and_then(Value::as_str).map(str::to_string))
        {
            let (_, page_a) = emulator.get_raw(id, Some(&token)).await;
            let (_, page_b) = emulator.get_raw(id, Some(&token)).await;
            assert_eq!(
                page_a, page_b,
                "case {case}: following the same nextToken twice must answer identically"
            );
        }

        let still_there = emulator
            .engine
            .state::<ComAmazonawsIamtoolboxAuthrequestserviceState>()
            .lock()
            .authorization_details
            .get(*id)
            .map(|record| record.evaluations.len());
        assert_eq!(
            still_there,
            Some(*count),
            "case {case}: reading must not consume or truncate the stored record"
        );
    }
}

/// The body is looked up by id, not canned: each seeded id answers from its own record,
/// and no two ids answer alike.
#[tokio::test]
async fn each_id_answers_from_its_own_record() {
    let cases: &[(&str, usize)] = &[("abc123def", 2), ("zzz999", 4), ("q7", 1)];
    let emulator = Emulator::new();
    for (id, count) in cases {
        emulator.record(record(id, *count));
    }

    let mut bodies: Vec<(String, Value)> = Vec::new();
    for (id, count) in cases {
        let case = format!("id={id} evaluations={count}");
        let (status, body) = emulator.get(id, None).await;
        assert_eq!(status, 200, "case {case}: must be 200, got {body}");
        assert_eq!(
            body,
            json!({
                "requestContext": request_context_json(id),
                "evaluations": (0..*count).map(|i| evaluation_json(id, i)).collect::<Vec<_>>(),
                "policies": policies_json(id),
            }),
            "case {case}: every member must be read out of this id's own record"
        );
        bodies.push(((*id).to_string(), body));
    }

    for (left_id, left) in &bodies {
        for (right_id, right) in &bodies {
            if left_id != right_id {
                assert_ne!(
                    left, right,
                    "case {left_id} vs {right_id}: two different records must answer two \
                     different bodies"
                );
            }
        }
    }
}

/// An id with no live record is `ResourceNotFoundException` 404 — never an empty 200 and
/// never a 501. The retention window the operation documents ("available for at least 24
/// hours after the denial") is the bound: inside it the record reads, past it the details
/// are gone.
#[tokio::test]
async fn an_id_with_no_live_record_is_not_found() {
    let emulator = Emulator::new();

    // Rows straddle the published 24h bound: well inside it, one second inside it, and
    // one second past it. "At least 24 hours" makes the bound itself still available,
    // so the row on the window is a 200 row.
    emulator.record(record("fresh", 3));
    emulator.record(
        record("nearlyexpired", 3).aged(AUTHORIZATION_DETAILS_RETENTION - Duration::from_secs(60)),
    );
    emulator.record(
        record("onesecondinside", 3).aged(AUTHORIZATION_DETAILS_RETENTION - Duration::from_secs(1)),
    );
    emulator.record(
        record("onesecondpast", 3).aged(AUTHORIZATION_DETAILS_RETENTION + Duration::from_secs(1)),
    );
    emulator.record(
        record("longexpired", 3).aged(AUTHORIZATION_DETAILS_RETENTION * 7),
    );

    let cases: &[(&str, u16)] = &[
        ("fresh", 200),
        ("nearlyexpired", 200),
        ("onesecondinside", 200),
        ("onesecondpast", 404),
        ("longexpired", 404),
        // Never recorded at all.
        ("nosuchid", 404),
        ("abc123def", 404),
    ];

    for (id, expected_status) in cases {
        let case = format!("id={id}");
        let (status, body) = emulator.get(id, None).await;
        assert_eq!(
            status, *expected_status,
            "case {case}: expected HTTP {expected_status}, got {status} with {body}"
        );
        if *expected_status == 404 {
            let (code, message) = error_of(&body, &case);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case}: expected ResourceNotFoundException, got {body}"
            );
            assert!(
                !message.is_empty(),
                "case {case}: the error must carry a message, got {body}"
            );
        }
    }

    // Recording also drops what the clock has already carried past the window, so the
    // store stays bounded by the retention window rather than by the number of denials
    // ever recorded. (`longexpired` was the last write, so its own sweep ran before it.)
    let stored = emulator
        .engine
        .state::<ComAmazonawsIamtoolboxAuthrequestserviceState>()
        .lock()
        .authorization_details
        .keys()
        .cloned()
        .collect::<Vec<String>>();
    for live in ["fresh", "nearlyexpired", "onesecondinside"] {
        assert!(
            stored.iter().any(|id| id == live),
            "case id={live}: a record inside the retention window must stay stored, \
             store holds {stored:?}"
        );
    }
    assert!(
        !stored.iter().any(|id| id == "onesecondpast"),
        "case id=onesecondpast: a later write must prune a record the clock expired, \
         store holds {stored:?}"
    );
}

/// A `nextToken` the service did not issue for the requested id is
/// `ValidationException` 400 with a message — whether it does not decode at all, carries
/// another version, was issued for another id, or names an offset no page of that
/// record's evaluations ever started at.
#[tokio::test]
async fn a_next_token_this_id_was_not_issued_is_a_validation_error() {
    let emulator = Emulator::new();
    emulator.record(record("abc123def", PAGE_SIZE + 2));
    emulator.record(record("zzz999", PAGE_SIZE + 2));
    // Exactly two full pages: its end offset IS page-aligned, so "aligned but never
    // issued" is reachable as its own row.
    emulator.record(record("twofullpages", PAGE_SIZE * 2));

    // The token the service issued for each id's second page: the only values the
    // reader is required to accept.
    let mut issued: HashMap<&str, String> = HashMap::new();
    for id in ["abc123def", "zzz999"] {
        let (status, body) = emulator.get(id, None).await;
        assert_eq!(status, 200, "setup id={id}: must be 200, got {body}");
        let token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("setup id={id}: a further page must carry a nextToken"))
            .to_string();
        issued.insert(id, token);
    }

    // Accepted: its own token. Refused: everything else.
    let mine = issued["abc123def"].clone();
    let other = issued["zzz999"].clone();
    let cases: Vec<(&str, &str, String, u16)> = vec![
        ("its own issued token", "abc123def", mine.clone(), 200),
        ("not a token at all", "abc123def", "garbage".to_string(), 400),
        ("empty", "abc123def", String::new(), 400),
        (
            "a token issued for another authorization id",
            "abc123def",
            other,
            400,
        ),
        // Hex that decodes to text, but not to a token this operation issues.
        (
            "decodes but is not a token",
            "abc123def",
            hex::encode("hello"),
            400,
        ),
        (
            "an unknown token version",
            "abc123def",
            hex::encode("v2:abc123def:10"),
            400,
        ),
        (
            "too many fields",
            "abc123def",
            hex::encode("v1:abc123def:10:10"),
            400,
        ),
        (
            "a non-numeric offset",
            "abc123def",
            hex::encode("v1:abc123def:ten"),
            400,
        ),
        (
            "a negative offset",
            "abc123def",
            hex::encode("v1:abc123def:-10"),
            400,
        ),
        (
            "an offset no page boundary fell on",
            "abc123def",
            hex::encode("v1:abc123def:3"),
            400,
        ),
        (
            "the first page's offset, which is never issued as a token",
            "abc123def",
            hex::encode("v1:abc123def:0"),
            400,
        ),
        (
            "an offset past the end of the evaluations",
            "abc123def",
            hex::encode("v1:abc123def:90"),
            400,
        ),
        (
            "a page-aligned offset at the end of the evaluations",
            "twofullpages",
            hex::encode(format!("v1:twofullpages:{}", PAGE_SIZE * 2)),
            400,
        ),
        (
            "a well-formed token for an id with no record",
            "nosuchid",
            hex::encode("v1:nosuchid:10"),
            404,
        ),
    ];

    for (what, id, token, expected_status) in cases {
        let case = format!("nextToken: {what} (id={id})");
        let (status, body) = emulator.get(id, Some(&token)).await;
        assert_eq!(
            status, expected_status,
            "case {case}: expected HTTP {expected_status}, got {status} with {body}"
        );
        if expected_status == 400 {
            let (code, message) = error_of(&body, &case);
            assert_eq!(
                code, "ValidationException",
                "case {case}: expected ValidationException, got {body}"
            );
            assert!(
                !message.is_empty(),
                "case {case}: the error must carry a message, got {body}"
            );
        }
    }

    // The accepted token still resolves to the record's second page.
    let (status, body) = emulator.get("abc123def", Some(&mine)).await;
    assert_eq!(status, 200, "the issued token must resolve, got {body}");
    assert_eq!(
        body.get("evaluations").and_then(Value::as_array).map(Vec::len),
        Some(2),
        "the issued token must resume at the stored offset, got {body}"
    );
}
