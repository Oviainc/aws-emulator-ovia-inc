// SPDX-License-Identifier: CC0-1.0

//! What an `Evaluate` result REPORTS about the span data it was computed from, and which
//! evaluator a request may ask for.
//!
//! The rules under test:
//!
//! * a result's `context.spanContext` identifies the span data the result was computed
//!   from: the session it belongs to, the trace it belongs to whenever all of that data
//!   belongs to ONE trace — whether the request named that trace or not — and the span
//!   when the evaluated unit is a single span (`SpanContext{sessionId required, traceId,
//!   spanId}`, source: smithy_query query="shape" shape="SpanContext");
//! * a span of the evaluated context that arrived without its corresponding log event
//!   leaves the result unscored and in-band at HTTP 200, with the incomplete-span-data
//!   message templated on THAT span: its own 16-hex id and the name it states
//!   (`errorMessage` is `EvaluationErrorMessage`, `@length(max 2048)`, source:
//!   smithy_query query="shape" shape="EvaluationResultContent");
//! * the evaluator the URI names is resolved BEFORE anything the body asks for: a
//!   `Builtin.` id that names no built-in evaluator the service provides is a 400
//!   `ValidationException` saying so, an id of the customer-evaluator family that names
//!   no created evaluator is a 404 `ResourceNotFoundException` saying so, and both come
//!   back whatever else is wrong with the request.
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator serves
//! with, so the routes, status codes and `restJson1` bodies are exercised and not just
//! the handler function.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;

/// The session every fixture span states under the `session.id` attribute.
const SESSION: &str = "session-7f3";

/// Two trace ids, each the 32 hexadecimal characters `TraceId` models, and three span
/// ids, each the 16 `SpanId` models (source: smithy_query query="shape" shape="TraceId",
/// shape="SpanId").
const TRACE_A: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
const TRACE_B: &str = "0af7651916cd43dd8448eb211c80319c";
const SPAN_A1: &str = "00f067aa0ba902b7";
const SPAN_A2: &str = "b7ad6b7169203331";
const SPAN_B1: &str = "a2fb4a1d1a96d312";

/// The same first trace id in the other hexadecimal case: one id, two spellings, and a
/// result reports the one the span it read states.
const TRACE_A_UPPER: &str = "4BF92F3577B34DA6A3CE929D0E0E4736";

/// A span id of the modelled shape that no fixture span carries.
const UNKNOWN_SPAN: &str = "ffffffffffffffff";

/// A built-in evaluator this service provides, used wherever a case is about something
/// other than the evaluator.
const EVALUATOR: &str = "Builtin.Correctness";

/// The `errorCode` a result carries when a span of the context arrived without its log
/// event.
const LOG_EVENTS_MISSING: &str = "LogEventMissingException";

/// The longest `errorMessage` a result may carry (`EvaluationErrorMessage`,
/// `@length(max 2048)`, source: smithy_query query="shape"
/// shape="EvaluationResultContent").
const MAX_ERROR_MESSAGE: usize = 2048;

/// The emulator, with request authentication off so a test signs nothing.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(agentcore::EmulatedService::new(engine));
    router
}

/// Send one request and return its status and parsed JSON body (`Value::Null` for an
/// empty body). The SigV4 credential scope is what routes the request to this service.
async fn send(router: &MultiServiceRouter, path: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://bedrock-agentcore.us-east-1.amazonaws.com{path}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/bedrock-agentcore/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// `Evaluate`: `POST /evaluations/evaluate/{evaluatorId}`.
async fn evaluate(router: &MultiServiceRouter, evaluator_id: &str, body: Value) -> (u16, Value) {
    send(
        router,
        &format!("/evaluations/evaluate/{evaluator_id}"),
        body,
    )
    .await
}

fn text<'a>(body: &'a Value, name: &str) -> &'a str {
    body.get(name).and_then(Value::as_str).unwrap_or("")
}

fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// The results a 200 answered with.
fn results(body: &Value) -> Vec<Value> {
    body.get("evaluationResults")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `spanContext` one result reports.
fn context(result: &Value) -> Value {
    result
        .get("context")
        .and_then(|context| context.get("spanContext"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// How a span states its own name, in each member spelling this operation reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NameMember {
    /// OpenTelemetry's own member.
    Otlp,
    /// The span-prefixed spellings an exporter that flattens a span into a record writes.
    LowerCamel,
    Snake,
    /// A span that states no name at all.
    Unnamed,
}

impl NameMember {
    fn member(self) -> Option<&'static str> {
        match self {
            Self::Otlp => Some("name"),
            Self::LowerCamel => Some("spanName"),
            Self::Snake => Some("span_name"),
            Self::Unnamed => None,
        }
    }
}

/// One span as a request carries it: `encoding` picks between the OTLP/JSON spelling
/// (lowerCamelCase members, attributes as typed `{key, value}` entries) and the
/// snake_case one other exporters write (attributes as a plain object), and `name` picks
/// which member — if any — states the span's name.
fn span(
    otlp: bool,
    span_id: &str,
    trace_id: &str,
    start: u64,
    name: (NameMember, &str),
    attributes: &[(&str, &str)],
) -> Value {
    let mut member = Map::new();
    if otlp {
        member.insert("spanId".to_string(), json!(span_id));
        member.insert("traceId".to_string(), json!(trace_id));
        member.insert("startTimeUnixNano".to_string(), json!(start.to_string()));
        member.insert(
            "attributes".to_string(),
            json!(attributes
                .iter()
                .map(|(key, value)| json!({"key": key, "value": {"stringValue": value}}))
                .collect::<Vec<Value>>()),
        );
    } else {
        member.insert("span_id".to_string(), json!(span_id));
        member.insert("trace_id".to_string(), json!(trace_id));
        member.insert("start_time_unix_nano".to_string(), json!(start));
        member.insert(
            "attributes".to_string(),
            Value::Object(
                attributes
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), json!(value)))
                    .collect::<Map<String, Value>>(),
            ),
        );
    }
    if let Some(spelling) = name.0.member() {
        member.insert(spelling.to_string(), json!(name.1));
    }
    Value::Object(member)
}

/// A span that records what the agent did, so its log event is there.
fn answering_span(otlp: bool, span_id: &str, trace_id: &str, start: u64) -> Value {
    span(
        otlp,
        span_id,
        trace_id,
        start,
        (NameMember::Otlp, "agent.answer"),
        &[("session.id", SESSION), ("gen_ai.completion", "an answer")],
    )
}

/// A span that records neither a tool call nor an answer, so its log event is missing.
fn silent_span(
    otlp: bool,
    span_id: &str,
    trace_id: &str,
    start: u64,
    name: (NameMember, &str),
) -> Value {
    span(
        otlp,
        span_id,
        trace_id,
        start,
        name,
        &[("session.id", SESSION)],
    )
}

/// A request body: the spans to evaluate and what to target.
fn request(spans: Vec<Value>, target: Option<Value>) -> Value {
    let mut body = json!({"evaluationInput": {"sessionSpans": spans}});
    if let Some(target) = target {
        body["evaluationTarget"] = target;
    }
    body
}

/// The incomplete-span-data message AWS words a span with no corresponding log event
/// with: fixed text, with the span's id and name substituted. A span that states no name
/// has no value to substitute, and the service spells an absent context value `None`.
fn incomplete_span_message(span_id: &str, span_name: Option<&str>) -> String {
    format!(
        "Session span data is incomplete. Span with ID: {span_id} and name: {} is missing a \
         corresponding log event. Please retry the request with the complete span data",
        span_name.unwrap_or("None")
    )
}

/// One row of [`a_result_reports_the_context_of_the_span_data_it_was_computed_from`]: the
/// case name, the spans sent, the target sent, and the (sessionId, traceId, spanId) the
/// single answered result must report.
type ContextRow = (
    &'static str,
    Vec<Value>,
    Option<Value>,
    (&'static str, Option<&'static str>, Option<&'static str>),
);

/// Every result reports the context of the span data it was computed from: the session,
/// the trace when all that data belongs to one, and the span when the unit is one span.
#[tokio::test]
async fn a_result_reports_the_context_of_the_span_data_it_was_computed_from() {
    let router = emulator();

    // Rows differ along how the unit is addressed (nothing targeted, one trace, one span,
    // a span no submitted span carries), how many spans and how many TRACES the evaluated
    // data covers, the span encoding, and the hexadecimal case the ids arrive in.
    let cases: Vec<ContextRow> = vec![
        (
            "whole-session-of-one-trace-reports-that-trace",
            vec![
                answering_span(true, SPAN_A1, TRACE_A, 1),
                answering_span(false, SPAN_A2, TRACE_A, 2),
            ],
            None,
            (SESSION, Some(TRACE_A), None),
        ),
        (
            "whole-session-of-one-span-reports-that-trace",
            vec![answering_span(true, SPAN_A1, TRACE_A, 1)],
            None,
            (SESSION, Some(TRACE_A), None),
        ),
        (
            "whole-session-of-one-trace-spelled-in-both-cases",
            vec![
                answering_span(true, SPAN_A1, TRACE_A, 1),
                answering_span(false, SPAN_A2, TRACE_A_UPPER, 2),
            ],
            None,
            (SESSION, Some(TRACE_A), None),
        ),
        (
            "whole-session-of-two-traces-reports-neither",
            vec![
                answering_span(true, SPAN_A1, TRACE_A, 1),
                answering_span(false, SPAN_B1, TRACE_B, 2),
            ],
            None,
            (SESSION, None, None),
        ),
        (
            "a-targeted-trace-reports-the-trace-it-names",
            vec![
                answering_span(true, SPAN_A1, TRACE_A, 1),
                answering_span(false, SPAN_B1, TRACE_B, 2),
            ],
            Some(json!({"traceIds": [TRACE_B]})),
            (SESSION, Some(TRACE_B), None),
        ),
        (
            "a-targeted-span-reports-its-own-trace-and-span",
            vec![
                answering_span(true, SPAN_A1, TRACE_A, 1),
                answering_span(false, SPAN_B1, TRACE_B, 2),
            ],
            Some(json!({"spanIds": [SPAN_B1]})),
            (SESSION, Some(TRACE_B), Some(SPAN_B1)),
        ),
        (
            "a-targeted-span-whose-trace-id-arrived-in-upper-case",
            vec![span(
                true,
                SPAN_A1,
                TRACE_A_UPPER,
                1,
                (NameMember::Otlp, "agent.turn"),
                &[("session.id", SESSION), ("tool.name", "search")],
            )],
            Some(json!({"spanIds": [SPAN_A1]})),
            (SESSION, Some(TRACE_A_UPPER), Some(SPAN_A1)),
        ),
        (
            "a-targeted-span-no-submitted-span-carries-reports-no-trace",
            vec![answering_span(true, SPAN_A1, TRACE_A, 1)],
            Some(json!({"spanIds": [UNKNOWN_SPAN]})),
            (SESSION, None, Some(UNKNOWN_SPAN)),
        ),
    ];

    for (case, spans, target, (session_id, trace_id, span_id)) in cases {
        let (status, answered) = evaluate(&router, EVALUATOR, request(spans, target)).await;
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
        let reported = context(&answered_results[0]);
        let actual = (
            reported.get("sessionId").and_then(Value::as_str),
            reported.get("traceId").and_then(Value::as_str),
            reported.get("spanId").and_then(Value::as_str),
        );
        assert_eq!(
            actual,
            (Some(session_id), trace_id, span_id),
            "case {case:?}: expected the context of the evaluated span data, got {reported:?}"
        );
    }
}

/// One row of [`a_span_with_no_log_event_is_reported_in_band_by_its_own_id_and_name`]:
/// the case name, the spans sent, the target sent, and the span id and name the message
/// must be templated on.
type IncompleteRow = (
    &'static str,
    Vec<Value>,
    Option<Value>,
    &'static str,
    Option<&'static str>,
);

/// A span of the evaluated context that arrived without its log event leaves the result
/// unscored at HTTP 200, with the incomplete-span-data message naming that span.
#[tokio::test]
async fn a_span_with_no_log_event_is_reported_in_band_by_its_own_id_and_name() {
    let router = emulator();

    // Rows differ along which member states the span's name (every spelling this
    // operation reads, plus a span that states none), the span encoding, how the unit is
    // addressed, and WHICH span of the unit is the one missing its log event — the only
    // one, the first of several, and one that follows a span whose log event is there.
    let cases: Vec<IncompleteRow> = vec![
        (
            "the-only-span-of-the-session-names-itself",
            vec![silent_span(
                true,
                SPAN_A1,
                TRACE_A,
                1,
                (NameMember::Otlp, "probe.workflow"),
            )],
            None,
            SPAN_A1,
            Some("probe.workflow"),
        ),
        (
            "a-name-stated-in-the-lower-camel-span-member",
            vec![silent_span(
                true,
                SPAN_A2,
                TRACE_A,
                1,
                (NameMember::LowerCamel, "probe.step"),
            )],
            None,
            SPAN_A2,
            Some("probe.step"),
        ),
        (
            "a-name-stated-in-the-snake-case-span-member",
            vec![silent_span(
                false,
                SPAN_B1,
                TRACE_B,
                1,
                (NameMember::Snake, "probe.tool"),
            )],
            None,
            SPAN_B1,
            Some("probe.tool"),
        ),
        (
            "a-span-that-states-no-name",
            vec![silent_span(false, SPAN_A1, TRACE_A, 1, (NameMember::Unnamed, ""))],
            None,
            SPAN_A1,
            None,
        ),
        (
            "the-first-of-two-silent-spans-in-trace-order",
            vec![
                silent_span(true, SPAN_A2, TRACE_A, 9, (NameMember::Otlp, "probe.second")),
                silent_span(true, SPAN_A1, TRACE_A, 4, (NameMember::Otlp, "probe.first")),
            ],
            None,
            SPAN_A1,
            Some("probe.first"),
        ),
        (
            "a-targeted-span-that-is-silent-beside-one-that-answered",
            vec![
                answering_span(true, SPAN_A1, TRACE_A, 1),
                silent_span(true, SPAN_A2, TRACE_A, 2, (NameMember::Otlp, "probe.step")),
            ],
            Some(json!({"spanIds": [SPAN_A2]})),
            SPAN_A2,
            Some("probe.step"),
        ),
        (
            "a-targeted-span-no-submitted-span-carries-has-no-name-to-state",
            vec![answering_span(true, SPAN_A1, TRACE_A, 1)],
            Some(json!({"spanIds": [UNKNOWN_SPAN]})),
            UNKNOWN_SPAN,
            None,
        ),
    ];

    for (case, spans, target, span_id, span_name) in cases {
        let (status, answered) = evaluate(&router, EVALUATOR, request(spans, target)).await;
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
        let expected = incomplete_span_message(span_id, span_name);
        let message = text(result, "errorMessage");
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
        assert_eq!(
            text(result, "errorCode"),
            LOG_EVENTS_MISSING,
            "case {case:?}: expected the unscored result to say why, got {result:?}"
        );
        assert!(
            message.chars().count() <= MAX_ERROR_MESSAGE,
            "case {case:?}: expected a message within EvaluationErrorMessage's \
             {MAX_ERROR_MESSAGE} characters, got {} characters",
            message.chars().count()
        );
        for leaked in ["parent_span_id", "session_id=", "trace_id=", "span_id="] {
            assert!(
                !message.contains(leaked),
                "case {case:?}: the message must not state {leaked:?}, got {message:?}"
            );
        }
    }

    // A span name is as long as the caller made it, and `errorMessage` admits 2048
    // characters: the NAME is what gives way, so the id and the remediation the caller
    // acts on survive.
    let name = "n".repeat(MAX_ERROR_MESSAGE * 2);
    let (status, answered) = evaluate(
        &router,
        EVALUATOR,
        request(
            vec![silent_span(
                true,
                SPAN_A1,
                TRACE_A,
                1,
                (NameMember::Otlp, name.as_str()),
            )],
            None,
        ),
    )
    .await;
    let case = "a-span-name-longer-than-the-message-admits";
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {answered:?}"
    );
    let message = text(&results(&answered)[0], "errorMessage").to_string();
    assert_eq!(
        message.chars().count(),
        MAX_ERROR_MESSAGE,
        "case {case:?}: expected the message held to {MAX_ERROR_MESSAGE} characters, got {}",
        message.chars().count()
    );
    assert!(
        message.starts_with(&format!(
            "Session span data is incomplete. Span with ID: {SPAN_A1} and name: n"
        )) && message.ends_with("Please retry the request with the complete span data"),
        "case {case:?}: expected the span id and the remediation to survive, got {message:?}"
    );
}

/// A context NO submitted span belongs to has no span to name, so the result says that
/// instead of dumping the lookup it was resolved by. The recorded wording covers the
/// span-level case, so what is required here is that the message states the fact about the
/// submitted span data and names none of the emulator's own lookup fields.
#[tokio::test]
async fn a_context_with_no_submitted_span_is_reported_without_naming_one() {
    let router = emulator();

    // Rows differ along how the context with no span data is addressed: a targeted trace
    // no span belongs to, and a reference input naming a session the spans do not state.
    let mut addressed_by_reference = request(vec![answering_span(true, SPAN_A1, TRACE_A, 1)], None);
    addressed_by_reference["evaluationReferenceInputs"] = json!([{
        "context": {"spanContext": {"sessionId": "another-session"}},
        "expectedResponse": {"text": "an answer"},
    }]);
    let cases: Vec<(&str, Value)> = vec![
        (
            "a-targeted-trace-no-submitted-span-belongs-to",
            request(
                vec![answering_span(true, SPAN_A1, TRACE_A, 1)],
                Some(json!({"traceIds": [TRACE_B]})),
            ),
        ),
        (
            "a-reference-input-naming-a-session-the-spans-do-not-state",
            addressed_by_reference,
        ),
    ];

    for (case, body) in cases {
        let (status, answered) = evaluate(&router, EVALUATOR, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answered:?}"
        );
        let result = &results(&answered)[0];
        let message = text(result, "errorMessage");
        assert_eq!(
            text(result, "errorCode"),
            LOG_EVENTS_MISSING,
            "case {case:?}: expected the unscored result to say why, got {result:?}"
        );
        assert!(
            message.starts_with("Session span data is incomplete.")
                && message.ends_with("Please retry the request with the complete span data"),
            "case {case:?}: expected the incomplete-span-data wording, got {message:?}"
        );
        for leaked in ["parent_span_id", "session_id=", "trace_id=", "span_id="] {
            assert!(
                !message.contains(leaked),
                "case {case:?}: the message must not state {leaked:?}, got {message:?}"
            );
        }
    }
}

/// One row of [`an_evaluator_that_names_nothing_is_refused_before_the_body_is_read`]: the
/// case name, the `evaluatorId` sent, the body sent, and the status, error code and
/// message expected.
type EvaluatorRow = (
    &'static str,
    &'static str,
    Value,
    u16,
    &'static str,
    String,
);

/// The evaluator the URI names is resolved before anything the body asks for: an id that
/// names no evaluator is refused with that id's own refusal, whatever else is wrong with
/// the request.
#[tokio::test]
async fn an_evaluator_that_names_nothing_is_refused_before_the_body_is_read() {
    let router = emulator();

    // A body that is itself unevaluable — one span stating no session at all — so a row
    // whose evaluator is refused proves the ORDER and not just the refusal.
    let unevaluable = request(
        vec![span(
            true,
            SPAN_A1,
            TRACE_A,
            1,
            (NameMember::Otlp, "agent.turn"),
            &[],
        )],
        None,
    );
    let evaluable = request(vec![answering_span(true, SPAN_A1, TRACE_A, 1)], None);

    // Rows differ along the evaluator family (a built-in name that is not provided, a
    // customer-evaluator id that was never created) and the form the id takes within it:
    // an unknown word, a provided name in another case (ids are exact), a provided name
    // with a suffix, a hyphenated name, and a customer id at each end of `EvaluatorId`'s
    // 1..111 length.
    let long_custom = format!("{}-0a1b2c3d4e", "c".repeat(100));
    let cases: Vec<EvaluatorRow> = vec![
        (
            "a-builtin-name-the-service-does-not-provide",
            "Builtin.NotAnEvaluator",
            unevaluable.clone(),
            400,
            "ValidationException",
            "Builtin evaluator Builtin.NotAnEvaluator does not exist. Please provide valid \
             Builtin evaluator name and retry."
                .to_string(),
        ),
        (
            "a-provided-builtin-name-in-another-case",
            "Builtin.correctness",
            evaluable.clone(),
            400,
            "ValidationException",
            "Builtin evaluator Builtin.correctness does not exist. Please provide valid \
             Builtin evaluator name and retry."
                .to_string(),
        ),
        (
            "a-provided-builtin-name-with-a-suffix",
            "Builtin.Correctness.v2",
            evaluable.clone(),
            400,
            "ValidationException",
            "Builtin evaluator Builtin.Correctness.v2 does not exist. Please provide valid \
             Builtin evaluator name and retry."
                .to_string(),
        ),
        (
            "a-hyphenated-builtin-name-the-service-does-not-provide",
            "Builtin.Goal-Accuracy",
            evaluable.clone(),
            400,
            "ValidationException",
            "Builtin evaluator Builtin.Goal-Accuracy does not exist. Please provide valid \
             Builtin evaluator name and retry."
                .to_string(),
        ),
        (
            "a-custom-evaluator-no-control-plane-created",
            "quality-checker-1a2b3c4d5e",
            unevaluable.clone(),
            404,
            "ResourceNotFoundException",
            "Custom evaluator not found: quality-checker-1a2b3c4d5e".to_string(),
        ),
        (
            "the-shortest-custom-evaluator-id-the-pattern-admits",
            "a-0123456789",
            evaluable.clone(),
            404,
            "ResourceNotFoundException",
            "Custom evaluator not found: a-0123456789".to_string(),
        ),
    ];

    for (case, evaluator_id, body, status, code, message) in cases {
        let (answered_status, answered) = evaluate(&router, evaluator_id, body).await;
        assert_eq!(
            answered_status, status,
            "case {case:?}: expected HTTP {status}, got {answered_status} with {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            code,
            "case {case:?}: expected {code}, got {answered:?}"
        );
        assert_eq!(
            text(&answered, "message"),
            message,
            "case {case:?}: expected message {message:?}, got {answered:?}"
        );
    }

    // The longest customer-evaluator id the pattern admits is refused the same way, and
    // the refusal carries the id exactly as the request spelled it.
    let (status, answered) = evaluate(&router, &long_custom, evaluable.clone()).await;
    assert_eq!(
        (status, error_code(&answered), text(&answered, "message")),
        (
            404,
            "ResourceNotFoundException",
            format!("Custom evaluator not found: {long_custom}").as_str()
        ),
        "case \"the-longest-custom-evaluator-id-the-pattern-admits\": expected the custom \
         evaluator to be reported missing, got {status} with {answered:?}"
    );

    // And an evaluator the service DOES provide evaluates the same body, so the refusals
    // above are about the id and not about the operation: a built-in one this emulator
    // provides, and a partner id, whose family no model of this service publishes a
    // catalogue of.
    for provided in [EVALUATOR, "Builtin.Helpfulness", "ThirdParty.DeepEval.Faithfulness"] {
        let (status, answered) = evaluate(&router, provided, evaluable.clone()).await;
        assert_eq!(
            status, 200,
            "case {provided:?}: a provided evaluator must evaluate, got {status} with {answered:?}"
        );
        assert_eq!(
            text(&results(&answered)[0], "evaluatorId"),
            provided,
            "case {provided:?}: the result must report the evaluator the request named, got \
             {answered:?}"
        );
    }
}
