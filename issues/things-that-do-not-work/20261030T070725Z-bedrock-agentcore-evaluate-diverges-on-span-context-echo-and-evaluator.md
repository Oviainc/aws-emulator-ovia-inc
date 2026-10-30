Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-evaluate-builtin-helpfulness
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-evaluate-builtin-helpfulness bedrock-agentcore-evaluate-error-paths
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore|Evaluate|2xx->2xx|$.evaluationResults[*].context.spanContext.traceId
Issue-Key: bedrock-agentcore|Evaluate|2xx->2xx|$.evaluationResults[*].errorMessage
Issue-Key: bedrock-agentcore|Evaluate|4xx->4xx|$.message
Issue-Key: bedrock-agentcore|Evaluate|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore|Evaluate|4xx->4xx|status
Issue-Commit: 64ec4a8c6a18a942a08809bdfbf9df5edbabd2ef

# Evaluate diverges on span-context echo and evaluator-existence refusals

> _Note: this issue was identified against the code base at commit `64ec4a8c6a18a942a08809bdfbf9df5edbabd2ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Evaluate

## Description
The bedrock-agentcore data-plane Evaluate operation (POST /evaluations/evaluate/{evaluatorId}) diverges from AWS in two related places: it does not resolve the evaluated span context (so the per-result context it echoes is incomplete and its errorMessage is emulator-composed prose rather than AWS's incomplete-span-data text), and it performs no evaluator-existence check at all, so both the custom-evaluator and builtin-evaluator refusal paths are replaced by a single generic 'no session' ValidationException. This bundle has 4 CAUSES; all four must be closed.

Cause 1 of 4 - the per-result spanContext is not populated from the request (fixture bedrock-agentcore-evaluate-builtin-helpfulness, step task#1).
AWS: Smithy: Evaluate output EvaluateResponse.evaluationResults (required) -> EvaluationResultContent, whose `context` member (required) is the Context union with member spanContext -> SpanContext{sessionId (required), traceId, spanId}. AWS fills that SpanContext from the span being evaluated, so evaluationResults[0].context.spanContext.traceId carries the 32-hex trace id of the span selected from evaluationInput.sessionSpans / evaluationTarget. Recorded response: the result's context.spanContext.traceId is present and equals the trace id of the submitted span.
Emulator: the member is absent -- $.evaluationResults[0].context.spanContext.traceId: missing (expected a 32-hex trace id, got None). The emulator emits a spanContext carrying only the session id it parsed, dropping traceId (and, by the same path, spanId).
Why: a client correlating an evaluation result back to the OTEL span/trace it was computed for has no trace id to join on, so result-to-trace attribution silently breaks and per-trace dashboards attribute every result to nothing.
Fix: the Evaluate handler's span-context resolution plus the EvaluationResultContent serializer in the bedrock-agentcore data-plane crate: resolve the target span from evaluationInput.sessionSpans (filtered by evaluationTarget.spanIds/traceIds) and copy its trace id and span id into the emitted SpanContext alongside sessionId.
Traces: bedrock-agentcore-evaluate-builtin-helpfulness#task#1.

Cause 2 of 4 - errorMessage for an un-evaluable span is emulator-invented text instead of AWS's incomplete-span-data message (fixtures bedrock-agentcore-evaluate-builtin-helpfulness steps task#1 and task#2).
AWS: Smithy: EvaluationResultContent.errorMessage -> EvaluationErrorMessage (@length max 2048); the model does not state the text, so recorded behavior only. When the submitted span has no matching log event AWS still answers 200 and reports it in-band with the fixed text "Session span data is incomplete. Span with ID: <spanId> and name: <spanName> is missing a corresponding log event. Please retry the request with the complete span data". Fixed text: everything outside the two substitutions; variable parts: <spanId> is the 16-hex span id of the offending span from the request and <spanName> is that span's name attribute (the recordings show e.g. a workflow span and a step span of the probe agent -- those literal values are request data filling the template, not part of it).
Emulator: answers 200 but with its own diagnostic, "No log events found for context: session_id='<id>' parent_span_id=None trace_id=<id-or-None> span_id=None" -- a Python-repr dump of the emulator's internal lookup key. It names no span id and no span name, and leaks internal field names (parent_span_id, the None literals) that appear nowhere in the AWS API.
Why: the message is the only signal telling the caller WHICH span was incomplete and what to resubmit; a client that parses or surfaces it gets no span identity, and the AWS-documented remediation ("retry the request with the complete span data") is absent, so automated retry logic keyed on that text never fires.
Fix: the Evaluate handler's evaluation-result error reporting in the bedrock-agentcore data-plane crate -- when span-to-log-event resolution fails, emit AWS's incomplete-span-data message templated on the resolved span's id and name instead of dumping the internal lookup context.
Traces: bedrock-agentcore-evaluate-builtin-helpfulness#task#1, bedrock-agentcore-evaluate-builtin-helpfulness#task#2.

Cause 3 of 4 - a nonexistent CUSTOM evaluator is not refused with ResourceNotFoundException (fixture bedrock-agentcore-evaluate-error-paths, step task#1).
AWS: Smithy: Evaluate.evaluatorId is an @httpLabel, required, EvaluatorId @length 1..111 @pattern ^(Builtin\.[...]|ThirdParty\.[...]|[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10})$; Evaluate models ResourceNotFoundException (client, httpError 404). An evaluatorId matching the customer-evaluator arm of the pattern that names no evaluator in the account is refused BEFORE any evaluation work: HTTP 404, header x-amzn-errortype "ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock.agentcore.common/", message "Custom evaluator not found: <evaluatorId>" -- fixed text "Custom evaluator not found: " plus the evaluatorId taken verbatim from the request path.
Emulator: answers 400 with x-amzn-errortype "ValidationException" and message "the evaluated context has no session: no span states a session.id attribute and no reference input supplies a session id." The existence pre-condition is never evaluated; the request instead fails later in context resolution.
Why: the wrong status class and error code -- a 400 ValidationException instead of a 404 ResourceNotFoundException -- flips the caller's decision: SDK retry/branching keyed on ResourceNotFoundException (create-the-evaluator-then-retry, or treat as permanent-not-found) never triggers, and the caller is told its payload is malformed when the real fault is a missing resource.
Fix: the Evaluate handler's validation/pre-condition ordering in the bedrock-agentcore data-plane crate -- look the evaluatorId up in evaluator state and raise ResourceNotFoundException with AWS's wording before resolving the evaluation context.
Traces: bedrock-agentcore-evaluate-error-paths#task#1.

Cause 4 of 4 - a nonexistent BUILTIN evaluator gets the generic no-session message instead of AWS's builtin-evaluator rejection (fixture bedrock-agentcore-evaluate-error-paths, step task#2).
AWS: Smithy: Evaluate.evaluatorId (EvaluatorId, @pattern Builtin\.[a-zA-Z0-9._-]+ arm) and ValidationException (client, httpError 400, members message required + reason required). A well-formed Builtin.* id that is not one of the service's builtin evaluators is refused with HTTP 400 ValidationException and the message "Builtin evaluator <evaluatorId> does not exist. Please provide valid Builtin evaluator name and retry." -- fixed text "Builtin evaluator " ... " does not exist. Please provide valid Builtin evaluator name and retry." with <evaluatorId> substituted from the request path (recording uses a Builtin.<name> id that does not exist).
Emulator: status 400 and errortype ValidationException are right, but the message is again "the evaluated context has no session: no span states a session.id attribute and no reference input supplies a session id." -- the request is rejected for the wrong reason and never mentions the unknown evaluator name.
Why: the caller cannot tell a bad evaluator name from a malformed evaluation payload; the actionable remediation (supply a valid builtin evaluator name) is lost, and any client asserting on the builtin-evaluator rejection text mis-handles the failure.
Fix: the Evaluate handler's validation in the bedrock-agentcore data-plane crate -- validate the Builtin.* evaluator name against the known builtin set and emit AWS's builtin-does-not-exist ValidationException message, ordered ahead of context resolution.
Traces: bedrock-agentcore-evaluate-error-paths#task#2.

No open issue covers Evaluate: the existing bedrock-agentcore message-fidelity issues (20261029T170914Z-bedrock-agentcore-resourcenotfoundexception-message-text-and-resolution-order-wrong-across.md and 20261029T215145Z-bedrock-agentcore-memory-data-plane-refusal-fidelity-validationexception-messages-are.md) are scoped to memory and code-interpreter lookups and carry no Evaluate replay key.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200):

Cause 1 - bedrock-agentcore-evaluate-builtin-helpfulness#task#1, Evaluate, expected 200 / actual 200:
  $.evaluationResults[0].context.spanContext.traceId: missing (expected a 32-hex trace id of the submitted span, got None)

Cause 2 - same step, plus bedrock-agentcore-evaluate-builtin-helpfulness#task#2 (expected 200 / actual 200):
  $.evaluationResults[0].errorMessage: value_mismatch
    expected: 'Session span data is incomplete. Span with ID: <spanId> and name: <spanName> is missing a corresponding log event. Please retry the request with the complete span data'
    actual:   "No log events found for context: session_id='<sessionId>' parent_span_id=None trace_id=<traceId|None> span_id=None"

Cause 3 - bedrock-agentcore-evaluate-error-paths#task#1, Evaluate:
  status: expected 404, got 400
  $headers.x-amzn-errortype: expected 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock.agentcore.common/', got 'ValidationException'
  $.message: expected 'Custom evaluator not found: <evaluatorId>', got 'the evaluated context has no session: no span states a session.id attribute and no reference input supplies a session id.'

Cause 4 - bedrock-agentcore-evaluate-error-paths#task#2, Evaluate, expected 400 / actual 400:
  $.message: expected 'Builtin evaluator <Builtin.EvaluatorName> does not exist. Please provide valid Builtin evaluator name and retry.', got 'the evaluated context has no session: no span states a session.id attribute and no reference input supplies a session id.'

Model evidence (services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/smithy-model): Evaluate POST /evaluations/evaluate/{evaluatorId}, code 200; errors include ResourceNotFoundException (404) and ValidationException (400, message+reason required); EvaluationResultContent.context is a required Context union -> SpanContext{sessionId required, traceId, spanId}; EvaluationResultContent.errorMessage -> EvaluationErrorMessage @length max 2048.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1 + Cause 2: replay fixture bedrock-agentcore-evaluate-builtin-helpfulness steps task#1 and task#2 -- POST /evaluations/evaluate/Builtin.<Helpfulness evaluator> with evaluationInput.sessionSpans containing spans whose log events are absent. Observe 200 with evaluationResults[0].context.spanContext missing traceId, and errorMessage carrying the emulator's "No log events found for context: ..." repr instead of AWS's "Session span data is incomplete. Span with ID: ... is missing a corresponding log event..." text.
3. Cause 3: replay fixture bedrock-agentcore-evaluate-error-paths step task#1 -- POST /evaluations/evaluate/<custom-evaluator-id matching the customer arm of the EvaluatorId pattern but not existing>. Expect 404 ResourceNotFoundException 'Custom evaluator not found: <evaluatorId>'; observe 400 ValidationException with the no-session message.
4. Cause 4: replay fixture bedrock-agentcore-evaluate-error-paths step task#2 -- POST /evaluations/evaluate/Builtin.<nonexistent name>. Expect 400 ValidationException 'Builtin evaluator <evaluatorId> does not exist. Please provide valid Builtin evaluator name and retry.'; observe the same generic no-session message.
