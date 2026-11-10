// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `Evaluate`, the AgentCore on-demand evaluation surface.
//!
//! The rules under test:
//!
//! * every id a caller supplies identifies ONE unit of evaluation, so a request that
//!   repeats an id — a span id inside `evaluationInput.sessionSpans`, an id inside
//!   `evaluationTarget`, or two `evaluationReferenceInputs` entries addressing one
//!   context — is refused with a 400 `ValidationException` naming that member, instead
//!   of being evaluated;
//! * `evaluationTarget` chooses the unit: one result per targeted span id, one per
//!   targeted trace id, with nothing targeted one per context the reference inputs
//!   address, and with neither one for the whole session — in the order the request
//!   names them, each reporting the context it is about;
//! * no result carries a rating: a rating is an evaluator's output and no judge model
//!   runs in this emulator, so `value`, `label`, `explanation` and `tokenUsage` are
//!   absent from every result and `errorCode` says which of the two reasons applies —
//!   the context's log events were not among the submitted spans, or they were and
//!   nothing judged them. Supplying ground truth does not change that.
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator
//! serves with, so the routes, status codes, `restJson1` bodies and modelled error
//! codes are exercised and not just the handler function.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;

/// The session every fixture span belongs to, under the `session.id` attribute the
/// OpenTelemetry semantic conventions define.
const SESSION: &str = "session-01";

/// Two trace ids, each the 32 characters `TraceId` models (source: smithy_query
/// query="shape" shape="TraceId").
const TRACE_A: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
const TRACE_B: &str = "0af7651916cd43dd8448eb211c80319c";

/// Three span ids, each the 16 characters `SpanId` models.
const SPAN_A1: &str = "00f067aa0ba902b7";
const SPAN_A2: &str = "b7ad6b7169203331";
const SPAN_B1: &str = "a2fb4a1d1a96d312";

/// A built-in evaluator, named the way `EvaluatorId`'s first pattern alternative spells
/// one.
const EVALUATOR: &str = "Builtin.Correctness";

/// A trace id and a span id of the right modelled shape that no fixture span carries.
const UNKNOWN_TRACE: &str = "ffffffffffffffffffffffffffffffff";
const UNKNOWN_SPAN: &str = "ffffffffffffffff";

/// The `errorCode` a result carries when the submitted spans record nothing for the
/// context it is about.
const LOG_EVENTS_MISSING: &str = "LogEventMissingException";

/// The `errorCode` a result carries when the context's log events are there and nothing
/// judged them.
const NO_JUDGE_MODEL: &str = "NoJudgeModelAvailable";

/// What the fixture's second trace answered with, which is the session's last answer.
const SESSION_ANSWER: &str = "Filing is due on April 15";

/// What the fixture's first trace answered with.
const TRACE_A_ANSWER: &str = "The tax rate is 7 percent";

/// The emulator, with request authentication off so a test signs nothing.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(agentcore::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Send one request and return its status and parsed JSON body (`Value::Null` for an
/// empty body). The SigV4 credential scope is what routes the request to this service.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(bytes::Bytes::new, |v| bytes::Bytes::from(v.to_string()));
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://bedrock-agentcore.us-east-1.amazonaws.com{path}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/bedrock-agentcore/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// `Evaluate`: `POST /evaluations/evaluate/{evaluatorId}`.
///
/// The `@httpLabel` needs no escaping: `EvaluatorId`'s pattern admits only letters,
/// digits, `.`, `_` and `-`, every one of them in RFC 3986's unreserved set (source:
/// smithy_query query="shape" shape="EvaluatorId").
async fn evaluate(router: &MultiServiceRouter, evaluator_id: &str, body: Value) -> (u16, Value) {
    send(
        router,
        "POST",
        &format!("/evaluations/evaluate/{evaluator_id}"),
        Some(body),
    )
    .await
}

fn member<'a>(body: &'a Value, name: &str) -> &'a Value {
    body.get(name).unwrap_or(&Value::Null)
}

fn text<'a>(body: &'a Value, name: &str) -> &'a str {
    member(body, name).as_str().unwrap_or("")
}

fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// The results a 200 answered with.
fn results(body: &Value) -> &Vec<Value> {
    static EMPTY: Vec<Value> = Vec::new();
    body.get("evaluationResults")
        .and_then(Value::as_array)
        .unwrap_or(&EMPTY)
}

/// One span in the OTLP/JSON encoding: lowerCamelCase members, and attributes as the
/// list of typed `{key, value}` entries that encoding uses.
fn otlp_span(span_id: &str, trace_id: &str, start: u64, attributes: &[(&str, &str)]) -> Value {
    json!({
        "name": "agent.turn",
        "spanId": span_id,
        "traceId": trace_id,
        "startTimeUnixNano": start.to_string(),
        "attributes": attributes
            .iter()
            .map(|(key, value)| json!({"key": key, "value": {"stringValue": value}}))
            .collect::<Vec<Value>>(),
    })
}

/// The same span as other exporters write it: snake_case members, a numeric start
/// instant, and attributes as a plain JSON object.
fn plain_span(span_id: &str, trace_id: &str, start: u64, attributes: &[(&str, &str)]) -> Value {
    json!({
        "name": "agent.turn",
        "span_id": span_id,
        "trace_id": trace_id,
        "start_time_unix_nano": start,
        "attributes": attributes
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect::<Map<String, Value>>(),
    })
}

/// The trace every case is run over: a tool call and an answer on trace A, and a second
/// tool call and answer on trace B, written in both span encodings so one request
/// exercises both.
fn fixture_spans() -> Vec<Value> {
    vec![
        otlp_span(
            SPAN_A1,
            TRACE_A,
            1,
            &[("session.id", SESSION), ("gen_ai.tool.name", "search")],
        ),
        otlp_span(
            SPAN_A2,
            TRACE_A,
            2,
            &[("session.id", SESSION), ("gen_ai.completion", TRACE_A_ANSWER)],
        ),
        plain_span(
            SPAN_B1,
            TRACE_B,
            3,
            &[
                ("session.id", SESSION),
                ("tool.name", "lookup"),
                ("output.value", SESSION_ANSWER),
            ],
        ),
    ]
}

/// A request body: the spans to evaluate, what to target, and the ground truth.
fn request(spans: Vec<Value>, target: Option<Value>, references: Vec<Value>) -> Value {
    let mut body = json!({"evaluationInput": {"sessionSpans": spans}});
    if let Some(target) = target {
        body["evaluationTarget"] = target;
    }
    if !references.is_empty() {
        body["evaluationReferenceInputs"] = json!(references);
    }
    body
}

/// One reference input addressing a context, with whatever ground truth it supplies.
fn reference(
    trace_id: Option<&str>,
    span_id: Option<&str>,
    expected_response: Option<&str>,
    assertions: &[&str],
    tools: &[&str],
) -> Value {
    let mut context = json!({"sessionId": SESSION});
    if let Some(trace_id) = trace_id {
        context["traceId"] = json!(trace_id);
    }
    if let Some(span_id) = span_id {
        context["spanId"] = json!(span_id);
    }
    let mut entry = json!({"context": {"spanContext": context}});
    if let Some(expected) = expected_response {
        entry["expectedResponse"] = json!({"text": expected});
    }
    if !assertions.is_empty() {
        entry["assertions"] = json!(assertions
            .iter()
            .map(|text| json!({"text": text}))
            .collect::<Vec<Value>>());
    }
    if !tools.is_empty() {
        entry["expectedTrajectory"] = json!({"toolNames": tools});
    }
    entry
}

/// The `spanContext` one result reports.
fn context(result: &Value) -> &Value {
    result
        .get("context")
        .and_then(|context| context.get("spanContext"))
        .unwrap_or(&Value::Null)
}

/// Assert that one result carries no rating of any kind, whatever reason it states.
fn assert_unrated(case: &str, result: &Value) {
    for absent in ["value", "label", "explanation", "tokenUsage"] {
        assert!(
            member(result, absent).is_null(),
            "case {case:?}: a result must carry no {absent}, got {result:?}"
        );
    }
}

/// One row of [`each_addressed_unit_is_reported_once_in_the_order_it_was_named`]: what a
/// failing assertion calls it, the `evaluationTarget` sent, the reference inputs sent,
/// and the (trace id, span id) every answered result must report, in order.
type ReportedContext = (Option<&'static str>, Option<&'static str>);
type TargetRow = (
    &'static str,
    Option<Value>,
    Vec<Value>,
    Vec<ReportedContext>,
);

/// One row of [`no_result_carries_a_rating_and_each_result_says_why`]: the case name,
/// the spans sent, the target sent, the reference inputs sent, and the `errorCode` the
/// result must carry.
type UnratedRow = (
    &'static str,
    Vec<Value>,
    Option<Value>,
    Vec<Value>,
    &'static str,
);

/// One row of [`a_repeated_id_is_refused_whichever_list_repeats_it`]: the case name, the
/// member the refusal must name, the request sent, and the message AWS answers it with
/// where that wording is recorded.
type RepeatedIdRow = (&'static str, &'static str, Value, Option<String>);

/// The wording a repeated reference-input context is refused with: the context's
/// members, each in single quotes where the request stated one and as `None` where it
/// did not. `SpanContext` carries no parent span id, so that member is never stated.
fn duplicate_context_message(trace_id: Option<&str>, span_id: Option<&str>) -> String {
    let quoted = |value: Option<&str>| {
        value.map_or_else(|| "None".to_string(), |value| format!("'{value}'"))
    };
    format!(
        "Duplicate context found: session_id='{SESSION}' parent_span_id=None trace_id={} \
         span_id={}",
        quoted(trace_id),
        quoted(span_id)
    )
}

/// Whatever list of ids a request repeats, the request names one unit twice and is
/// refused with a 400 `ValidationException` naming that member.
#[tokio::test]
async fn a_repeated_id_is_refused_whichever_list_repeats_it() {
    let (router, _engine) = emulator();

    // Rows differ along WHICH list repeats an id (the submitted spans, the two target
    // unions, the reference inputs at each of the three context levels) and along the
    // form of the repetition (the same spelling, and the other case of a hexadecimal
    // id), plus the span encoding the repetition is written in. The last element is the
    // message the refusal must state where AWS's own wording for it is known.
    let cases: Vec<RepeatedIdRow> = vec![
        (
            "two-spans-sharing-one-span-id",
            "evaluationInput.sessionSpans",
            request(
                vec![
                    otlp_span(SPAN_A1, TRACE_A, 1, &[("session.id", SESSION)]),
                    otlp_span(SPAN_A1, TRACE_A, 2, &[("session.id", SESSION)]),
                ],
                None,
                vec![],
            ),
            None,
        ),
        (
            "two-spans-sharing-one-span-id-in-the-other-case",
            "evaluationInput.sessionSpans",
            request(
                vec![
                    otlp_span(SPAN_A1, TRACE_A, 1, &[("session.id", SESSION)]),
                    plain_span(
                        &SPAN_A1.to_uppercase(),
                        TRACE_A,
                        2,
                        &[("session.id", SESSION)],
                    ),
                ],
                None,
                vec![],
            ),
            None,
        ),
        (
            "target-span-ids-naming-one-span-twice",
            "evaluationTarget.spanIds",
            request(
                fixture_spans(),
                Some(json!({"spanIds": [SPAN_A1, SPAN_A1]})),
                vec![],
            ),
            None,
        ),
        (
            "target-span-ids-naming-one-span-in-both-cases",
            "evaluationTarget.spanIds",
            request(
                fixture_spans(),
                Some(json!({"spanIds": [SPAN_A1, SPAN_A1.to_uppercase()]})),
                vec![],
            ),
            None,
        ),
        (
            "target-trace-ids-naming-one-trace-twice",
            "evaluationTarget.traceIds",
            request(
                fixture_spans(),
                Some(json!({"traceIds": [TRACE_B, TRACE_B]})),
                vec![],
            ),
            None,
        ),
        (
            "two-session-level-reference-inputs",
            "evaluationReferenceInputs",
            request(
                fixture_spans(),
                None,
                vec![
                    reference(None, None, Some("one"), &[], &[]),
                    reference(None, None, Some("two"), &[], &[]),
                ],
            ),
            Some(duplicate_context_message(None, None)),
        ),
        (
            "two-trace-level-reference-inputs-for-one-trace",
            "evaluationReferenceInputs",
            request(
                fixture_spans(),
                Some(json!({"traceIds": [TRACE_A]})),
                vec![
                    reference(Some(TRACE_A), None, Some("one"), &[], &[]),
                    reference(Some(TRACE_A), None, None, &["two"], &[]),
                ],
            ),
            Some(duplicate_context_message(Some(TRACE_A), None)),
        ),
        (
            "two-span-level-reference-inputs-for-one-span",
            "evaluationReferenceInputs",
            request(
                fixture_spans(),
                Some(json!({"spanIds": [SPAN_A2]})),
                vec![
                    reference(Some(TRACE_A), Some(SPAN_A2), Some("one"), &[], &[]),
                    reference(Some(TRACE_A), Some(SPAN_A2), Some("two"), &[], &[]),
                ],
            ),
            Some(duplicate_context_message(Some(TRACE_A), Some(SPAN_A2))),
        ),
    ];

    for (case, refused_member, body, expected_message) in cases {
        let (status, answered) = evaluate(&router, EVALUATOR, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answered:?}"
        );
        assert_eq!(
            text(&answered, "reason"),
            "FieldValidationFailed",
            "case {case:?}: expected reason FieldValidationFailed, got {answered:?}"
        );
        let named: Vec<&str> = member(&answered, "fieldList")
            .as_array()
            .map(|fields| fields.iter().map(|field| text(field, "name")).collect())
            .unwrap_or_default();
        assert_eq!(
            named,
            vec![refused_member],
            "case {case:?}: expected fieldList to name {refused_member:?}, got {answered:?}"
        );
        let message = text(&answered, "message");
        match expected_message {
            Some(expected) => assert_eq!(
                message, expected,
                "case {case:?}: expected message {expected:?}, got {message:?}"
            ),
            // No recorded wording for the other lists, so what is required of them is
            // that the refusal says which member it is about and which entries of it.
            None => assert!(
                message.contains(refused_member)
                    && (message.contains("[1]") || message.contains("index 1")),
                "case {case:?}: expected the message to name {refused_member:?} and the \
                 repeated entry, got {message:?}"
            ),
        }
    }
}

/// With every supplied id its own, the request is evaluated: one result per unit it
/// addresses, in the order it addresses them, each reporting the context it is about.
#[tokio::test]
async fn each_addressed_unit_is_reported_once_in_the_order_it_was_named() {
    let (router, _engine) = emulator();

    // One row per level the target addresses, plus a row that reverses the order of the
    // ids, a row that targets one span of a trace, and rows that target nothing at all —
    // where the units are the contexts the reference inputs address.
    let cases: Vec<TargetRow> = vec![
        (
            "no-target-and-a-session-level-reference-input",
            None,
            vec![reference(None, None, Some(SESSION_ANSWER), &[], &[])],
            vec![(None, None)],
        ),
        (
            "no-target-and-no-reference-input-is-the-whole-session",
            None,
            vec![],
            vec![(None, None)],
        ),
        (
            "no-target-and-a-trace-level-reference-input",
            None,
            vec![reference(Some(TRACE_A), None, Some(TRACE_A_ANSWER), &[], &[])],
            vec![(Some(TRACE_A), None)],
        ),
        (
            "no-target-and-a-reference-input-per-trace",
            None,
            vec![
                reference(Some(TRACE_B), None, Some(SESSION_ANSWER), &[], &[]),
                reference(Some(TRACE_A), None, Some(TRACE_A_ANSWER), &[], &[]),
            ],
            vec![(Some(TRACE_B), None), (Some(TRACE_A), None)],
        ),
        (
            "two-traces",
            Some(json!({"traceIds": [TRACE_A, TRACE_B]})),
            vec![reference(None, None, Some(SESSION_ANSWER), &[], &[])],
            vec![(Some(TRACE_A), None), (Some(TRACE_B), None)],
        ),
        (
            "the-same-two-traces-in-the-other-order",
            Some(json!({"traceIds": [TRACE_B, TRACE_A]})),
            vec![reference(None, None, Some(SESSION_ANSWER), &[], &[])],
            vec![(Some(TRACE_B), None), (Some(TRACE_A), None)],
        ),
        (
            "three-spans-across-both-traces",
            Some(json!({"spanIds": [SPAN_B1, SPAN_A1, SPAN_A2]})),
            vec![reference(None, None, Some(SESSION_ANSWER), &[], &[])],
            vec![
                (Some(TRACE_B), Some(SPAN_B1)),
                (Some(TRACE_A), Some(SPAN_A1)),
                (Some(TRACE_A), Some(SPAN_A2)),
            ],
        ),
        (
            "one-span-of-one-trace",
            Some(json!({"spanIds": [SPAN_A2]})),
            vec![reference(None, None, Some(SESSION_ANSWER), &[], &[])],
            vec![(Some(TRACE_A), Some(SPAN_A2))],
        ),
    ];

    for (case, target, references, expected) in cases {
        let body = request(fixture_spans(), target, references);
        let (status, answered) = evaluate(&router, EVALUATOR, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answered:?}"
        );
        let answered_results = results(&answered);
        assert_eq!(
            answered_results.len(),
            expected.len(),
            "case {case:?}: expected {} result(s), got {answered:?}",
            expected.len()
        );
        for (at, (trace_id, span_id)) in expected.iter().enumerate() {
            let result = &answered_results[at];
            let reported = context(result);
            assert_eq!(
                reported.get("sessionId").and_then(Value::as_str),
                Some(SESSION),
                "case {case:?}: result {at} must report the session its spans state, got {result:?}"
            );
            assert_eq!(
                reported.get("traceId").and_then(Value::as_str),
                *trace_id,
                "case {case:?}: result {at} must report the trace it is about, got {result:?}"
            );
            assert_eq!(
                reported.get("spanId").and_then(Value::as_str),
                *span_id,
                "case {case:?}: result {at} must report the span it is about, got {result:?}"
            );
            assert_eq!(
                text(result, "evaluatorId"),
                EVALUATOR,
                "case {case:?}: result {at} must report the evaluator the request named, got {result:?}"
            );
            assert_unrated(case, result);
        }
    }
}

/// No result carries a rating, and each one says which of the two reasons applies: the
/// context's log events were not among the submitted spans, or they were and no judge
/// model ran over them.
#[tokio::test]
async fn no_result_carries_a_rating_and_each_result_says_why() {
    let (router, _engine) = emulator();

    // Rows differ along the level the unit sits at, how the context is addressed (a
    // target, a reference input, or nothing), what the spans record for it (a tool call,
    // an answer, both, or nothing), and how much ground truth the request supplies —
    // including the fullest ground truth the model allows, which is still not a rating.
    let cases: Vec<UnratedRow> = vec![
        (
            "the-whole-session-with-no-ground-truth",
            fixture_spans(),
            None,
            vec![],
            NO_JUDGE_MODEL,
        ),
        (
            "the-whole-session-with-every-kind-of-ground-truth",
            fixture_spans(),
            None,
            vec![reference(
                None,
                None,
                Some(SESSION_ANSWER),
                &["April 15", "no fee applies"],
                &["search", "lookup"],
            )],
            NO_JUDGE_MODEL,
        ),
        (
            "a-targeted-trace-that-answered",
            fixture_spans(),
            Some(json!({"traceIds": [TRACE_A]})),
            vec![reference(Some(TRACE_A), None, Some(TRACE_A_ANSWER), &[], &[])],
            NO_JUDGE_MODEL,
        ),
        (
            "a-targeted-span-that-called-a-tool",
            fixture_spans(),
            Some(json!({"spanIds": [SPAN_A1]})),
            vec![reference(Some(TRACE_A), Some(SPAN_A1), None, &[], &["search"])],
            NO_JUDGE_MODEL,
        ),
        (
            "a-targeted-span-no-submitted-span-carries",
            fixture_spans(),
            Some(json!({"spanIds": [UNKNOWN_SPAN]})),
            vec![reference(None, None, Some(SESSION_ANSWER), &[], &[])],
            LOG_EVENTS_MISSING,
        ),
        (
            "a-targeted-trace-no-submitted-span-belongs-to",
            fixture_spans(),
            Some(json!({"traceIds": [UNKNOWN_TRACE]})),
            vec![reference(None, None, Some(SESSION_ANSWER), &[], &["search"])],
            LOG_EVENTS_MISSING,
        ),
        (
            "a-reference-input-for-a-trace-the-spans-do-not-carry",
            fixture_spans(),
            None,
            vec![reference(
                Some(UNKNOWN_TRACE),
                None,
                Some(SESSION_ANSWER),
                &[],
                &[],
            )],
            LOG_EVENTS_MISSING,
        ),
        (
            "a-reference-input-for-a-session-the-spans-do-not-state",
            vec![otlp_span(
                SPAN_A1,
                TRACE_A,
                1,
                &[
                    ("session.id", "another-session"),
                    ("gen_ai.completion", SESSION_ANSWER),
                ],
            )],
            None,
            vec![reference(Some(TRACE_A), None, Some(SESSION_ANSWER), &[], &[])],
            LOG_EVENTS_MISSING,
        ),
        (
            "spans-of-the-context-that-record-neither-an-answer-nor-a-tool-call",
            vec![otlp_span(SPAN_A1, TRACE_A, 1, &[("session.id", SESSION)])],
            None,
            vec![reference(Some(TRACE_A), None, Some(SESSION_ANSWER), &[], &[])],
            LOG_EVENTS_MISSING,
        ),
    ];

    for (case, spans, target, references, expected_code) in cases {
        let body = request(spans, target, references);
        let (status, answered) = evaluate(&router, EVALUATOR, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answered:?}"
        );
        let answered_results = results(&answered);
        assert_eq!(
            answered_results.len(),
            1,
            "case {case:?}: expected one result, got {answered:?}"
        );
        let result = &answered_results[0];
        assert_unrated(case, result);
        assert_eq!(
            text(result, "errorCode"),
            expected_code,
            "case {case:?}: expected the unrated result to say why, got {result:?}"
        );
        let message = text(result, "errorMessage");
        assert!(
            !message.is_empty() && !message.to_lowercase().contains("duplicate"),
            "case {case:?}: expected a message about this result and not about a repeated \
             context, got {message:?}"
        );
        assert!(
            member(result, "ignoredReferenceInputFields").is_null(),
            "case {case:?}: nothing was compared, so no reference input field is singled \
             out as ignored, got {result:?}"
        );
    }
}

/// A request this operation cannot evaluate is refused, with the status and code the
/// model carries for that failure — never served with an invented result.
#[tokio::test]
async fn a_request_that_cannot_be_evaluated_is_refused() {
    let (router, _engine) = emulator();

    // Rows differ along what is wrong: a span that is not an object, a span with no id
    // of its own, a span whose start instant cannot be read, spans from which no session
    // can be read at all, and an evaluator id naming a custom evaluator no control plane
    // created here.
    let cases: Vec<(&str, &str, u16, &str, &str)> = vec![
        (
            "a-span-that-is-not-an-object",
            EVALUATOR,
            400,
            "ValidationException",
            "CannotParse",
        ),
        (
            "a-span-carrying-no-span-id",
            EVALUATOR,
            400,
            "ValidationException",
            "FieldValidationFailed",
        ),
        (
            "a-span-stating-an-unreadable-start-instant",
            EVALUATOR,
            400,
            "ValidationException",
            "CannotParse",
        ),
        (
            "spans-stating-no-session-with-no-reference-input",
            EVALUATOR,
            400,
            "ValidationException",
            "FieldValidationFailed",
        ),
        (
            "a-custom-evaluator-this-emulator-holds-none-of",
            "quality-checker-1a2b3c4d5e",
            404,
            "ResourceNotFoundException",
            "",
        ),
    ];

    for (case, evaluator_id, expected_status, expected_code, expected_reason) in cases {
        let body = match case {
            "a-span-that-is-not-an-object" => request(vec![json!("not a span")], None, vec![]),
            "a-span-carrying-no-span-id" => request(
                vec![json!({"traceId": TRACE_A, "attributes": {"session.id": SESSION}})],
                None,
                vec![],
            ),
            "a-span-stating-an-unreadable-start-instant" => request(
                vec![json!({
                    "spanId": SPAN_A1,
                    "traceId": TRACE_A,
                    "startTimeUnixNano": "2026-10-29T21:51:45Z",
                    "attributes": {"session.id": SESSION},
                })],
                None,
                vec![],
            ),
            "spans-stating-no-session-with-no-reference-input" => {
                request(vec![otlp_span(SPAN_A1, TRACE_A, 1, &[])], None, vec![])
            }
            _ => request(
                fixture_spans(),
                None,
                vec![reference(None, None, Some(SESSION_ANSWER), &[], &[])],
            ),
        };
        let (status, answered) = evaluate(&router, evaluator_id, body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            expected_code,
            "case {case:?}: expected {expected_code}, got {answered:?}"
        );
        if !expected_reason.is_empty() {
            assert_eq!(
                text(&answered, "reason"),
                expected_reason,
                "case {case:?}: expected reason {expected_reason}, got {answered:?}"
            );
        }
    }
}

/// A built-in and a partner evaluator are evaluators the service itself provides: a
/// result reports the id the request named, the evaluator's name within its family, and
/// the account-free ARN form `EvaluatorArn` publishes for them.
#[tokio::test]
async fn a_provided_evaluator_is_reported_by_its_own_id() {
    let (router, _engine) = emulator();

    // Rows differ along the evaluator's family, the punctuation its id carries (source:
    // smithy_query query="shape" shape="EvaluatorId" — `Builtin.<name>`,
    // `ThirdParty.<vendor>.<name>`) and whether the id fits `EvaluatorName`'s shorter
    // maximum of 48 characters, which the last row's 81-character id does not.
    for (evaluator_id, expected_name) in [
        ("Builtin.Correctness", "Builtin.Correctness"),
        ("Builtin.Helpfulness", "Builtin.Helpfulness"),
        ("Builtin.Goal.Accuracy", "Builtin.Goal.Accuracy"),
        (
            "ThirdParty.acme_labs.Faithfulness",
            "ThirdParty.acme_labs.Faithfulness",
        ),
        (
            "ThirdParty.a_very_long_vendor_name_for_evaluation.Faithfulness_With_A_Long_Suffix",
            "Faithfulness_With_A_Long_Suffix",
        ),
    ] {
        let body = request(
            fixture_spans(),
            None,
            vec![reference(None, None, Some(SESSION_ANSWER), &[], &[])],
        );
        let (status, answered) = evaluate(&router, evaluator_id, body).await;
        assert_eq!(
            status, 200,
            "case {evaluator_id:?}: expected HTTP 200, got {status} with {answered:?}"
        );
        let result = &results(&answered)[0];
        assert_eq!(
            (
                text(result, "evaluatorId"),
                text(result, "evaluatorName"),
                text(result, "evaluatorArn"),
            ),
            (
                evaluator_id,
                expected_name,
                format!("arn:aws:bedrock-agentcore:::evaluator/{evaluator_id}").as_str(),
            ),
            "case {evaluator_id:?}: expected the provided evaluator's own identity, got {result:?}"
        );
    }
}

/// Evaluating is a computation over the request: it stores nothing, so the same request
/// answers the same results however often it is sent, and no later caller sees anything
/// an evaluation left behind.
#[tokio::test]
async fn evaluating_changes_nothing_a_later_caller_sees() {
    let (router, engine) = emulator();

    let body = request(
        fixture_spans(),
        Some(json!({"traceIds": [TRACE_A, TRACE_B]})),
        vec![
            reference(None, None, Some(SESSION_ANSWER), &["April 15"], &[]),
            reference(Some(TRACE_A), None, Some(TRACE_A_ANSWER), &[], &["search"]),
        ],
    );

    let (first_status, first) = evaluate(&router, EVALUATOR, body.clone()).await;
    assert_eq!(
        first_status, 200,
        "expected HTTP 200 from the first call, got {first_status} with {first:?}"
    );
    for round in 1..=2 {
        let (status, again) = evaluate(&router, EVALUATOR, body.clone()).await;
        assert_eq!(
            (status, &again),
            (200, &first),
            "case {round:?}: expected the same request to answer the same results, got {status} with {again:?}"
        );
    }

    let state = agentcore::state::service_state(&*engine).expect("the service state");
    let store = state.lock();
    assert!(
        store.memories.is_empty() && store.code_interpreters.is_empty() && store.order.is_empty(),
        "an evaluation must leave no record in this service's state, got {} memory \
         container(s) and {} code interpreter(s)",
        store.memories.len(),
        store.code_interpreters.len()
    );
}
