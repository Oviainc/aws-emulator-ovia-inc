Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-runtime.invoke-model-malformed-json-body-rejected bedrock-runtime.malformed-request-body-not-forwarded-to-model bedrock-runtime.bedrock-runtime-converse-tool-result-with-an-unknown-tool-use-id-rejected
Issue-Shape-Id: com.amazonaws.bedrockruntime#AmazonBedrockFrontendService
Issue-Key: bedrock-runtime.invoke-model-malformed-json-body-rejected|run#4|AssertionError|assert message == 'Malformed input request, please reformat your input and try again.', f"unexpected refusal message {message!r}"
Issue-Key: bedrock-runtime.malformed-request-body-not-forwarded-to-model|run#11|AssertionError|assert message == "Malformed input request, please reformat your input and try again.", message
Issue-Key: bedrock-runtime.bedrock-runtime-converse-tool-result-with-an-unknown-tool-use-id-rejected|run#24|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Commit: 15d5f672973fe406b6b38447a2fe490e83bf1181

# bedrock-runtime refusal messages are re-worded by the emulator: malformed InvokeModel JSON body and Converse toolResult with an unknown toolUseId (2 causes)

> _Note: this issue was identified against the code base at commit `15d5f672973fe406b6b38447a2fe490e83bf1181`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeModel

## Description
Two net-new causes in bedrock-runtime's data-plane refusal path. In both, the HTTP status (400) and the error code (ValidationException) are correct, but the `message` member carries text the emulator authored rather than the sentence the service emits. The error message is a requirement, so a client, golden-file test or support tool that matches on the service's sentence cannot tell these refusals apart from any other 400.

NOTE on neighbouring issues: open issue 20261029T213834Z-bedrock-runtime-error-paths-diverge-wrong-status-and.md files the same CLASS of defect (re-worded validation messages) but for ApplyGuardrail's input-format refusal, InvokeGuardrailChecks' enum-set ordering and two async-invoke status/code defects. Neither cause below is among them, so that issue does not cover these; it is named here because the fix may live in the same shared validation/message layer. Open issue 20261027T061233Z (over-permissive input validation) is also distinct: here the call IS refused, only the wording diverges.

Cause 1 of 2: InvokeModel with a body that is not valid JSON is refused with the emulator's own sentence instead of AWS's.
AWS: Smithy com.amazonaws.bedrockruntime#InvokeModel (POST /model/{modelId}/invoke) models ValidationException with @httpError 400; `body` is the @httpPayload blob (@length max 25000000, @sensitive). When the payload is not parseable JSON for the target model, the service refuses at the API boundary — it is never forwarded to the model — with HTTP 400, error code ValidationException and the message 'Malformed input request, please reformat your input and try again.' That text is entirely FIXED: it names no member, value, offset or model id, so there is no variable part to fill from the request or from state (recorded behavior for the exact sentence; the model declares the code and status).
Emulator: HTTP 400 and ValidationException (both correct), but message = 'The JSON object in the request body is malformed. Fix the syntax errors and retry your request.' — a different, emulator-authored sentence.
Why: callers and negative tests pin on AWS's sentence to distinguish 'the body could not be parsed' from any other ValidationException on the same operation (bad modelId, out-of-range inferenceConfig, oversized payload). A re-worded message makes those indistinguishable, breaks message assertions and log-based alarms, and makes a test that passes against the emulator fail on AWS.
Fix: the InvokeModel handler's body-parse/validation stage in the bedrock-runtime crate — emit the service's fixed malformed-input sentence for the JSON-parse failure instead of a locally-written description of the syntax error.
Traces: bedrock-runtime.invoke-model-malformed-json-body-rejected, bedrock-runtime.malformed-request-body-not-forwarded-to-model (both exercise the same parse-failure path with different malformed payloads and both observe the same substituted sentence).

Cause 2 of 2: Converse with a toolResult addressed to a toolUseId the service never minted is refused with a re-worded, differently-shaped message.
AWS: Smithy com.amazonaws.bedrockruntime#Converse (POST /model/{modelId}/converse) models ValidationException with @httpError 400; the conversation history is carried in `messages` (Messages list) and tool wiring in `toolConfig`. When a toolResult block names a toolUseId that does not match the toolUse the service issued in the preceding assistant turn, Converse refuses with HTTP 400 / ValidationException and a message built from AWS's template: FIXED text 'Expected toolResult blocks at ' + '<message path>' + ' for the following Ids: ' + '<expected ids>' + ', but found: ' + '<found ids>'. The VARIABLE parts are (a) the path of the offending content list, taken from the request's message indexing (e.g. messages.<n>.content), (b) the toolUseId(s) the service minted in the prior turn, taken from emulator state, and (c) the toolUseId(s) actually present in the submitted toolResult block(s), taken from the request. Recorded behavior only for the exact template wording; the status and code are modeled.
Emulator: HTTP 400 and ValidationException (both correct), but the message is a prose instruction of its own composition — 'The toolResult block at <path>.0 has toolUseId <found>, but the tool request it can answer was issued with toolUseId <expected>. Try again with a toolResult whose toolUseId is the one of the toolUse block it answers.' It names the same two ids (so the identifier matching itself is right) but neither the fixed text nor the ordering/shape matches AWS's template, and it points at an indexed block rather than the content list.
Why: a tool-use agent loop distinguishes 'my toolResult is addressed to the wrong id' from other history-shape refusals by matching the 'Expected toolResult blocks at ... for the following Ids: ... but found: ...' sentence and by parsing the expected-id list out of it to re-issue the turn. The emulator's sentence defeats both the match and the parse, so recovery logic that works against AWS does not fire here.
Fix: the Converse handler's tool-history validation (the stage that reconciles toolResult.toolUseId against the minted toolUse ids) — raise the ValidationException using AWS's template with the content-list path and the expected/found id lists filled from the request and state.
Traces: bedrock-runtime.bedrock-runtime-converse-tool-result-with-an-unknown-tool-use-id-rejected.

## Evidence
Behavioral replay, all three checks verdict=mismatch; in every case status and error code were correct and only the `message` member diverged.

Cause 1:
- bedrock-runtime.invoke-model-malformed-json-body-rejected — InvokeModel(modelId=amazon.nova-lite-v1:0, contentType=application/json, body=b'this is not json at all {{{'). code: ValidationException (as expected). message: expected 'Malformed input request, please reformat your input and try again.', got 'The JSON object in the request body is malformed. Fix the syntax errors and retry your request.' (AssertionError: unexpected refusal message ...).
- bedrock-runtime.malformed-request-body-not-forwarded-to-model — InvokeModel with body=b'{"messages": [{"role": "user", "content": [{maxTokenCount: 20,,,}'. code: ValidationException (as expected). message: expected 'Malformed input request, please reformat your input and try again.', got 'The JSON object in the request body is malformed. Fix the syntax errors and retry your request.' — byte-identical substituted sentence, confirming one shared parse-error site rather than two.

Cause 2:
- bedrock-runtime.bedrock-runtime-converse-tool-result-with-an-unknown-tool-use-id-rejected — a forced toolChoice turn mints a toolUseId; the history is replayed with its last character mutated. code: ValidationException (as expected), and both ids appear in the text, but the message diverges:
  expected: 'Expected toolResult blocks at messages.2.content for the following Ids: <minted>, but found: <mutated>'
  got:      'The toolResult block at messages.2.content.0 has toolUseId <mutated>, but the tool request it can answer was issued with toolUseId <minted>. Try again with a toolResult whose toolUseId is the one of the toolUse block it answers.'
  (The concrete ids in the recorded run — tooluse_ac607e244584202993a659 / ...a65A — are per-run values that fill the template's variable slots; the requirement is the fixed text and the slot order, not those literals. 'messages.2' likewise follows from the request's own message indexing.)

Smithy (services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json): InvokeModel errors include ValidationException errorKind=client httpError=400, input member `body` = @httpPayload blob @length max 25000000; Converse errors include ValidationException errorKind=client httpError=400, input members `messages` (Messages) and `toolConfig` (ToolConfiguration).

## Steps to Reproduce
1. git clone the repo and boot the emulator (run_behavioral_suite builds and boots it).
2. Cause 1: call InvokeModel on amazon.nova-lite-v1:0 with contentType=application/json and a body that is not valid JSON (e.g. b'this is not json at all {{{', or b'{"messages": [{"role": "user", "content": [{maxTokenCount: 20,,,}'). Observe HTTP 400 / ValidationException with message 'The JSON object in the request body is malformed. Fix the syntax errors and retry your request.' instead of 'Malformed input request, please reformat your input and try again.' Re-replay behavioral checks invoke-model-malformed-json-body-rejected and malformed-request-body-not-forwarded-to-model.
3. Cause 2: call Converse on amazon.nova-lite-v1:0 with a toolConfig whose toolChoice forces one tool, take the minted toolUseId from the assistant turn, then re-send [user turn, assistant turn, user turn carrying a toolResult] with the toolResult.toolUseId altered by one character. Observe HTTP 400 / ValidationException whose message is the emulator's prose sentence instead of 'Expected toolResult blocks at messages.2.content for the following Ids: <minted id>, but found: <submitted id>'. Re-replay behavioral check bedrock-runtime-converse-tool-result-with-an-unknown-tool-use-id-rejected.
