Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-runtime.guardrail-cannot-be-bypassed-via-parameter-manipulation bedrock-runtime.converse-tool-use-full-cycle bedrock-runtime.bedrock-runtime-converse-stream-tool-use-input-arrives-only-in-deltas bedrock-runtime.model-runtime-failure-mapped-to-model-error-exception bedrock-runtime.invoke-model-response-stream-frames-verify-their-own-crc32 bedrock-runtime.bedrock-runtime-invoke-model-stream-invocation-metrics-match-nonstreaming-usage bedrock-runtime.invoke-model-missing-permission-denied bedrock-runtime.invoke-model-unknown-model-id-not-found bedrock-runtime.empty-vs-absent-optional-field-distinguished bedrock-runtime.binary-blob-base64-round-trip-preserved
Issue-Shape-Id: com.amazonaws.bedrockruntime#AmazonBedrockFrontendService
Issue-Key: bedrock-runtime.guardrail-cannot-be-bypassed-via-parameter-manipulation|run#7|AssertionError|assert exact_resp["stopReason"] == "guardrail_intervened", (
Issue-Key: bedrock-runtime.converse-tool-use-full-cycle|run#5|AssertionError|assert first["stopReason"] == "tool_use", (
Issue-Key: bedrock-runtime.bedrock-runtime-converse-stream-tool-use-input-arrives-only-in-deltas|run#6|AssertionError|assert stops == ["tool_use"], (
Issue-Key: bedrock-runtime.model-runtime-failure-mapped-to-model-error-exception|_error_code#6|AssertionError|raise AssertionError(f"expected the call to be refused, but it SUCCEEDED and returned {result!r}")
Issue-Key: bedrock-runtime.invoke-model-response-stream-frames-verify-their-own-crc32|run#21|AssertionError|assert len(frames) >= 2, (
Issue-Key: bedrock-runtime.bedrock-runtime-invoke-model-stream-invocation-metrics-match-nonstreaming-usage|run#7|AssertionError|assert sum(1 for o in decoded if _METRICS_KEY in o) == 1, (
Issue-Key: bedrock-runtime.invoke-model-missing-permission-denied|_poll#18|Timeout|time.sleep(interval_s)
Issue-Key: bedrock-runtime.invoke-model-unknown-model-id-not-found|run#4|AssertionError|assert invoke_code == "ValidationException", f"expected ValidationException, got code={invoke_code}"
Issue-Key: bedrock-runtime.empty-vs-absent-optional-field-distinguished|_error#8|AssertionError|raise AssertionError("call unexpectedly succeeded")
Issue-Key: bedrock-runtime|InvokeModel|ModelErrorException
Issue-Commit: cd7a0e788c437c5439a918b59f215cef2324a883

# bedrock-runtime synchronous inference: guardrailConfig and toolConfig silently ignored, no authorization gate, response stream is a single frame, wrong not-found class

> _Note: this issue was identified against the code base at commit `cd7a0e788c437c5439a918b59f215cef2324a883`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Converse

## Description
SEVEN CAUSES on the synchronous Converse / InvokeModel / InvokeModelWithResponseStream surface. Each is a distinct contract a fixer must see answer correctly. (Smithy model read for all seven: services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json.) These are all independent of what text a model generates - none of them is an assertion on generated content.

Cause 1 of 7 - Converse ignores `guardrailConfig`: a guardrail that must intervene does not (OVER-PERMISSIVE, safety-relevant).
AWS: Smithy ConverseRequest has member `guardrailConfig` -> GuardrailConfiguration, and ConverseResponse's required `stopReason` is the StopReason enum whose values include `guardrail_intervened`. When a configured policy matches, the turn stops with stopReason='guardrail_intervened' and the content is replaced by the operator's blockedInputMessaging / blockedOutputsMessaging (both @required, @length 1..500, on CreateGuardrail in the control-plane model).
Emulator: the call returns 200 with stopReason='end_turn' and the content unaltered - the guardrail is accepted on the request and has no effect on the response.
Why: a client that relies on Bedrock guardrails for content safety gets NO enforcement while the API reports success, so forbidden content flows straight through to end users. This is the single highest-impact divergence in this issue.
Fix: the Converse/InvokeModel handler's guardrail evaluation stage (apply the referenced guardrail's policies to input and output, set stopReason and substitute the configured messaging).
Traces: bedrock-runtime.guardrail-cannot-be-bypassed-via-parameter-manipulation.

Cause 2 of 7 - Converse/ConverseStream ignore `toolConfig.toolChoice`: stopReason is never `tool_use`.
AWS: Smithy ToolChoice is a union {auto, any, tool}; `any` is documented as "The model must request at least one tool (no text is generated)" and `tool` as "The Model must request the specified tool". With either, the response's required stopReason is `tool_use` (a StopReason enum value) and the output message carries a toolUse content block; on ConverseStream the tool input arrives in contentBlockDelta frames and messageStop carries stopReason='tool_use'. A well-formed request whose token budget cannot complete a forced tool-use turn surfaces as ModelErrorException (Smithy: errorKind client, httpError 424) on Converse, never as a 200.
Emulator: stopReason='end_turn' with a plain text block; ConverseStream emits frames ['messageStart','contentBlockDelta','contentBlockStop','messageStop','metadata'] with messageStop stopReason='end_turn' and no toolUse; the forced-tool-with-tiny-maxTokens request returns 200 with stopReason='max_tokens' instead of 424 ModelErrorException.
Why: every agent/tool-calling client breaks - it waits for a toolUse block that never arrives, and a model-side failure is indistinguishable from a normal truncated answer.
Fix: the Converse/ConverseStream handler's toolConfig handling (honour toolChoice, emit a toolUse content block and the tool_use stop reason, and map the unsatisfiable forced-tool case to ModelErrorException/424).
Traces: bedrock-runtime.converse-tool-use-full-cycle, bedrock-runtime.bedrock-runtime-converse-stream-tool-use-input-arrives-only-in-deltas, bedrock-runtime.model-runtime-failure-mapped-to-model-error-exception.

Cause 3 of 7 - InvokeModelWithResponseStream returns ONE event-stream frame carrying the whole unary body, and never an invocationMetrics chunk.
AWS: Smithy InvokeModelWithResponseStream output binds `body` -> ResponseStream (@streaming union with members chunk/internalServerException/modelStreamErrorException/validationException/throttlingException/modelTimeoutException/serviceUnavailableException) as the httpPayload, and `contentType` to the httpHeader X-Amzn-Bedrock-Content-Type. The body is a sequence of application/vnd.amazon.eventstream messages: one `chunk` per generated delta, and a terminal chunk carrying the `amazon-bedrock-invocationMetrics` key exactly once (recorded behavior only - the metrics key is injected by the service and is not a modeled member). Multiple frames, each with its own prelude CRC-32 and message CRC-32, are the requirement.
Emulator: the 660-byte body parses into exactly 1 frame, and that frame's payload keys are ['output','stopReason','usage'] - i.e. the NON-streaming Converse/InvokeModel response serialized once into a single chunk; `amazon-bedrock-invocationMetrics` appears 0 times.
Why: a streaming client gets no incremental deltas (defeating the whole point of the operation) and no token/latency metrics chunk, so per-invocation accounting is impossible; code that expects >=2 frames or reads the metrics chunk fails.
Fix: the InvokeModelWithResponseStream serializer / event-stream framing (emit per-delta chunks plus a terminal invocationMetrics chunk), shared with the ConverseStream framing.
Traces: bedrock-runtime.invoke-model-response-stream-frames-verify-their-own-crc32, bedrock-runtime.bedrock-runtime-invoke-model-stream-invocation-metrics-match-nonstreaming-usage.

Cause 4 of 7 - no authorization gate on InvokeModel/Converse: a principal with NO bedrock:InvokeModel grant is served (OVER-PERMISSIVE).
AWS: Smithy lists AccessDeniedException (errorKind client, httpError 403) on both InvokeModel and Converse. A signed call by an IAM user whose only Allow has been deleted is refused with AccessDeniedException; re-attaching the identical Allow restores it.
Emulator: after the Allow is deleted the real signed InvokeModel and Converse keep succeeding; the check's bounded poll for AccessDeniedException never converges and the check is killed at its 60 s budget. (Hypothesis: the hang is in that post-deletion poll - the two polls before it require SUCCESS and therefore cannot hang, so the timeout localises to the refusal that never came.)
Why: authorization is unobservable, so every negative-path test of a client's least-privilege policy passes against the emulator and fails against AWS; an emulator that never denies cannot be used to validate IAM posture.
Fix: the InvokeModel/Converse handlers' authorization check (consult the signing principal's effective policy before serving).
Traces: bedrock-runtime.invoke-model-missing-permission-denied.

Cause 5 of 7 - an unknown but well-formed foundation-model id is answered ResourceNotFoundException (404) instead of ValidationException (400).
AWS: Bedrock resolves base-model identifiers during request validation, so a well-formed-but-nonexistent foundation-model id is refused with ValidationException (Smithy: errorKind client, httpError 400); recorded message example 'The provided model identifier is invalid.' ResourceNotFoundException (404) is also modeled on InvokeModel but is reserved for other cases (e.g. an end-of-life model version).
Emulator: code 'ResourceNotFoundException' (404) for the same input.
Why: the status class flips from 400 to 404, so clients that branch on 'bad request vs resource gone' take the wrong branch, and retry/alarm policies keyed on 4xx sub-class misfire.
Fix: the InvokeModel/Converse model-identifier resolution error mapping.
Traces: bedrock-runtime.invoke-model-unknown-model-id-not-found.

Cause 6 of 7 - Converse accepts an explicitly EMPTY system text block (OVER-PERMISSIVE).
AWS: Smithy SystemContentBlock is a union whose `text` member targets NonEmptyString with @length min 1. An omitted `system` is fine, but `system=[{"text": ""}]` violates that constraint and is refused with ValidationException (400); the message is built from the request and the constraint, so the RULE is the requirement: "1 validation error detected: Value at 'system.1.member.text' failed to satisfy constraint: Member must have length greater than or equal to 1".
Emulator: the call succeeds (200). Absence and explicit emptiness are made interchangeable.
Why: a client shipping an accidentally empty system prompt is silently served here and refused in production; the length constraint is not enforced anywhere on this member.
Fix: Converse input validation (apply the @length constraint on SystemContentBlock.text, and the member-path message rule).
Traces: bedrock-runtime.empty-vs-absent-optional-field-distinguished.

Cause 7 of 7 - InvokeModel against an embedding model answers ModelErrorException instead of a 200 embedding response.
AWS: Smithy InvokeModel -> POST /model/{modelId}/invoke returns 200 with the required `body` blob and required `contentType` header; an embedding model returns a well-formed JSON body. ModelErrorException is modeled (errorKind client, httpError 424) for a model-side failure, not for a supported model serving a well-formed request.
Emulator: 424 ModelErrorException, message example 'No model runs in this Amazon Bedrock Runtime emulator, so it serves no embedding inference for amazon.titan-embed-image-v1.' The divergence filed here is the STATUS and error class (424 where AWS answers 200 with a body) - the embedding vector's numeric values are model output and are explicitly NOT the requirement.
Why: every client of the embedding data plane fails hard; nothing downstream (base64 blob round-trip, vector indexing) can be exercised at all.
Fix: the InvokeModel handler's embedding-model path (serve a 200 with a well-formed response body for embedding models).
Traces: bedrock-runtime.binary-blob-base64-round-trip-preserved.

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1, bedrock-runtime 11/63 passed. The emulator answered these calls (200s and modeled 4xx were returned), so the service is up and answering wrongly.

Cause 1 - bedrock-runtime.guardrail-cannot-be-bypassed-via-parameter-manipulation:
  AssertionError: expected guardrail_intervened for the exact forbidden phrase, got 'end_turn'

Cause 2 -
  bedrock-runtime.converse-tool-use-full-cycle: AssertionError: expected stopReason 'tool_use' with toolChoice=any; got 'end_turn' content=[{'text': ...}]
  bedrock-runtime.bedrock-runtime-converse-stream-tool-use-input-arrives-only-in-deltas: AssertionError: a mandatory toolChoice must produce exactly one messageStop with stopReason 'tool_use'; got ['end_turn'] over frames ['messageStart', 'contentBlockDelta', 'contentBlockStop', 'messageStop', 'metadata']
  bedrock-runtime.model-runtime-failure-mapped-to-model-error-exception: AssertionError: expected the call to be refused, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'output': {...}, 'stopReason': 'max_tokens', 'usage': {...}}

Cause 3 -
  bedrock-runtime.invoke-model-response-stream-frames-verify-their-own-crc32: AssertionError: a real response stream must carry at least a content delta and a terminal metrics frame; 660 body bytes parsed into 1 frame(s)
  bedrock-runtime.bedrock-runtime-invoke-model-stream-invocation-metrics-match-nonstreaming-usage: AssertionError: amazon-bedrock-invocationMetrics must appear exactly once; it appeared 0 times over keys [['output', 'stopReason', 'usage']]

Cause 4 - bedrock-runtime.invoke-model-missing-permission-denied:
  Error: Check timed out after 60.0s
  (the check creates an IAM user, attaches an Allow for bedrock:InvokeModel/bedrock:Converse, confirms both signed calls succeed, deletes the Allow, then polls up to 90 s for AccessDeniedException - the poll that cannot converge against a target with no authorization gate)

Cause 5 - bedrock-runtime.invoke-model-unknown-model-id-not-found:
  AssertionError: expected ValidationException, got code=ResourceNotFoundException

Cause 6 - bedrock-runtime.empty-vs-absent-optional-field-distinguished:
  AssertionError: call unexpectedly succeeded
  (request: Converse modelId amazon.nova-lite-v1:0, system=[{'text': ''}], client-side parameter validation disabled so the request reaches the service)

Cause 7 - bedrock-runtime.binary-blob-base64-round-trip-preserved:
  botocore.errorfactory.ModelErrorException: An error occurred (ModelErrorException) when calling the InvokeModel operation: No model runs in this Amazon Bedrock Runtime emulator, so it serves no embedding inference for amazon.titan-embed-image-v1.

Model evidence: smithy_query operation Converse/InvokeModel/InvokeModelWithResponseStream and shape SystemContentBlock/StopReason/ToolChoice/ResponseStream/TokenUsage on services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json - StopReason values [end_turn, tool_use, max_tokens, stop_sequence, guardrail_intervened, content_filtered, malformed_model_output, malformed_tool_use, model_context_window_exceeded]; ToolChoice union {auto, any, tool}; SystemContentBlock.text -> NonEmptyString @length min 1; ResponseStream @streaming union with member `chunk`; Converse/InvokeModel errors include AccessDeniedException(403), ModelErrorException(424), ResourceNotFoundException(404), ValidationException(400).

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Run the behavioral suite and re-replay the check ids in trace_ids.
3. Cause 1: CreateGuardrail with a wordPolicy blocking a phrase, publish a version, then Converse with guardrailConfig naming it and a prompt containing the phrase. Expected stopReason='guardrail_intervened' and the configured blocked messaging; observed stopReason='end_turn' and unaltered content.
4. Cause 2: Converse with toolConfig={'tools':[...], 'toolChoice':{'any':{}}}. Expected stopReason='tool_use' and a toolUse content block (and on ConverseStream, tool input only in contentBlockDelta frames); observed stopReason='end_turn' with a text block. Then repeat with a forced tool and maxTokens too small to complete it: expected 424 ModelErrorException, observed 200.
5. Cause 3: issue InvokeModelWithResponseStream and parse the raw body as application/vnd.amazon.eventstream. Expected >=2 frames and exactly one chunk carrying 'amazon-bedrock-invocationMetrics'; observed 1 frame whose payload is the unary body ['output','stopReason','usage'] and zero metrics chunks.
6. Cause 4: create an IAM user, attach an Allow for bedrock:InvokeModel/bedrock:Converse, confirm a signed InvokeModel succeeds, delete the Allow, then re-issue the signed InvokeModel. Expected 403 AccessDeniedException; observed continued success.
7. Cause 5: InvokeModel with modelId='anthropic.claude-nonexistent-model-v99'. Expected 400 ValidationException; observed 404 ResourceNotFoundException.
8. Cause 6: Converse with system=[{'text': ''}] and botocore parameter_validation disabled. Expected 400 ValidationException naming 'system.1.member.text'; observed 200.
9. Cause 7: InvokeModel with modelId='amazon.titan-embed-image-v1' and a well-formed embedding body. Expected 200 with a JSON body; observed 424 ModelErrorException.
