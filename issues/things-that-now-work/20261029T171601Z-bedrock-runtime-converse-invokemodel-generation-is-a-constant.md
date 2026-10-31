Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-runtime.converse-basic-text-roundtrip bedrock-runtime.converse-multiturn-context-retained bedrock-runtime.invoke-model-temperature-out-of-range-rejected bedrock-runtime.list-async-invokes-invalid-max-results-rejected bedrock-runtime.numeric-parameter-precision-preserved
Issue-Shape-Id: com.amazonaws.bedrockruntime#AmazonBedrockFrontendService
Issue-Key: bedrock-runtime.converse-basic-text-roundtrip|run#8|AssertionError|assert "pong" in full_text.lower(), f"expected PONG in reply, got: {full_text!r}"
Issue-Key: bedrock-runtime.converse-multiturn-context-retained|run#10|AssertionError|assert SECRET_NUMBER in text, f"expected {SECRET_NUMBER} recalled, got: {text!r}"
Issue-Key: bedrock-runtime.invoke-model-temperature-out-of-range-rejected|_error#3|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: bedrock-runtime.list-async-invokes-invalid-max-results-rejected|run#11|AssertionError|assert msg_token == _MSG_TOKEN, f"expected message {_MSG_TOKEN!r}, got {msg_token!r}"
Issue-Key: bedrock-runtime.numeric-parameter-precision-preserved|run#10|AssertionError|assert under_top_p_message == (
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# bedrock-runtime: Converse/InvokeModel generation is a constant notice string, out-of-range InvokeModel temperature is accepted with 200, and two validation refusals use the emulator's own wording (ListAsyncInvokes nextToken, inferenceConfig.topP float rendering)

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Resolution

Causes 2, 3 and 4 are FIXED: `InvokeModel` now validates its native body's inference parameters against the
ranges the invoked model's provider documents (400 `ValidationException` naming the parameter and its bounds),
`ListAsyncInvokes` refuses a pagination token it cannot place with the fixed `Invalid pagination token was
passed` and quotes none of the request back, and a `@range` clause quotes a submitted floating-point value the
way the service writes one (`-1.0E-6`, not `-0.000001`). Their three replay keys pass.

Cause 1 is NOT fixed and is NOT closed: it is a platform blocker, and it stays FILED — its defect and its two
replay keys (`bedrock-runtime.converse-basic-text-roundtrip`,
`bedrock-runtime.converse-multiturn-context-retained`) now live on the open report that owns that defect,
`issues/things-that-do-not-work/20261028T194707Z-bedrock-runtime-generation-stage-serves-a-constant-emulator.md`
(which this report's own Cause 1 named as the one describing it). Nothing in this emulator can run a language
model, and a stage that instead computes the text from the request was implemented and refused at the merge gate
as output synthesised to look computed; the generation stage therefore still serves the notice that says no
model ran.

## Operation
Converse

## Description
This issue bundles 4 DISTINCT root causes on the bedrock-runtime surface. A fixer must close ALL FOUR before submitting; each cause below names its own failing check ids. Model consulted: services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json. bedrock-runtime answered every call below (200s and modeled 400s were both returned), so the service is up and answering wrongly, not unbootstrapped.

---
Cause 1 of 4: the generation stage serves one constant notice string instead of prompt-responsive text, so neither a single-turn instruction nor prior conversation context is ever reflected in the completion.
AWS: Smithy Converse POST /model/{modelId}/converse -> 200 with @required output/stopReason/usage; generated text is not described by the model, so recorded behavior only: Converse on amazon.nova-lite-v1:0 with 'Reply with exactly the single word PONG and nothing else.' returns an assistant text block containing that word, and a second turn whose `messages` carry the first turn's user text and assistant reply answers with the value stated in that first turn (here a 4-digit number the check chose — a per-request value, not a literal). stopReason='end_turn' and usage.inputTokens/outputTokens > 0 with totalTokens = inputTokens + outputTokens.
Emulator: both turns return HTTP 200 with the single text block 'No language model runs in this emulator. amazon.nova-lite-v1:0 generated no text for this request.' — the instruction is ignored on turn one and the context carried in `messages` is ignored on turn two.
Why: every client that reads the completion (instruction following, multi-turn context assembly, extraction, agent loops) is untestable against the emulator; a context-assembly bug that drops prior turns produces exactly the same bytes as a correct client here.
Fix: the generation stage behind Converse/ConverseStream/InvokeModel in the bedrock-runtime frontend — it must derive text from the request's own messages (including prior turns) rather than returning a fixed notice. NOTE: the open issue 20261028T194707Z-bedrock-runtime-generation-stage-serves-a-constant-emulator.md describes this same defect but does NOT carry these two checks' replay keys; this cause exists to add them, not to describe a second defect.
Traces: bedrock-runtime.converse-basic-text-roundtrip, bedrock-runtime.converse-multiturn-context-retained.

---
Cause 2 of 4: InvokeModel accepts an inferenceConfig.temperature above the documented maximum and serves a 200 instead of refusing it.
AWS: InvokeModel's `body` is an opaque blob (Smithy: Body, @httpPayload, @sensitive, @length max 25000000), so the provider's native schema is not in the model; the equivalent modeled constraint is InferenceConfiguration.temperature -> Float @range {min:0, max:1} on Converse, the same [0.0, 1.0] contract the Nova native body carries. Recorded behavior for the exact text: InvokeModel on a Nova model with inferenceConfig.temperature=5.0 is refused with HTTP 400, Error.Code='ValidationException', Error.Message='Malformed input request: the provided temperature must be between 0.0 and 1.0, please reformat your input and try again.' — entirely fixed text for this error class (the bounds come from the model's documented range, not from the request). The in-range control (temperature=0.5, same model and prompt) still succeeds, so the refusal is boundary-specific.
Emulator: the out-of-range call SUCCEEDS with HTTP 200 (content-type application/json, x-amzn-bedrock-performanceconfig-latency: standard) and returns a body; the value is neither rejected nor clamped, just ignored.
Why: a caller that builds temperature from user input or config discovers the production 400 only in production, and a caller probing the valid range gets no signal at all — the native InvokeModel body is validated against the provider's inference-parameter contract nowhere.
Fix: the InvokeModel handler's native-body validation stage — apply the provider's inference-parameter ranges (temperature/topP in [0,1], maxTokens >= 1) to the decoded body and answer 400 ValidationException with the message above. NOTE: this is the same root cause as Cause 2 of the open issue 20261027T061233Z-bedrock-runtime-synchronous-and-async-inference-input-validation.md, which carries only the replay key of bedrock-runtime.invoke-model-parameter-out-of-range-rejected; this cause adds this check's key.
Traces: bedrock-runtime.invoke-model-temperature-out-of-range-rejected.

---
Cause 3 of 4: ListAsyncInvokes refuses a malformed nextToken with the emulator's own wording, and that wording echoes the rejected token back to the caller.
AWS: Smithy ListAsyncInvokes GET /async-invoke -> 200, nextToken -> PaginationToken @length {min:1,max:2048} @pattern ^\S*$, and ValidationException (errorKind client, httpError 400) is modeled. Recorded behavior for the exact text: a malformed nextToken is refused with HTTP 400, Error.Code='ValidationException', Error.Message='Invalid pagination token was passed' — the whole message is FIXED text for this error class; it contains no value from the request and in particular does not quote the rejected token. The maxResults legs of the same recording are a different message template whose only variable parts are the submitted value and the bound ('1 validation error detected: Value '<value>' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1' / 'less than or equal to 1000'), and the emulator already matches those.
Emulator: the code is right (ValidationException) but the message is 'The nextToken not-a-valid-token!! is not valid.' — the emulator's own sentence, with the rejected token interpolated into it.
Why: the message is part of the contract: clients and operators match or log on it, so the divergence breaks message-sensitive handling; and echoing the submitted token adds a request value to a message AWS keeps constant, which also reflects caller-controlled bytes back into error output.
Fix: the ListAsyncInvokes handler's pagination-token validation — answer the fixed 'Invalid pagination token was passed' message for an undecodable/unknown nextToken, leaving the maxResults range messages as they are.
Traces: bedrock-runtime.list-async-invokes-invalid-max-results-rejected.

---
Cause 4 of 4: the @range violation message for Converse's inferenceConfig.topP renders the submitted float as a plain decimal expansion, where AWS renders it the way the service formats a float, so a small-magnitude value produces a different message.
AWS: Smithy InferenceConfiguration.topP -> Float @range {min:0, max:1} (and temperature the same), with ValidationException (client, 400) modeled on Converse. Recorded behavior for the exact text: Converse with inferenceConfig.topP=-1e-6 is refused with Error.Code='ValidationException' and Error.Message='1 validation error detected: Value '-1.0E-6' at 'inferenceConfig.topP' failed to satisfy constraint: Member must have value greater than or equal to 0'. The fixed text is '1 validation error detected: Value ' / ' at ' / ' failed to satisfy constraint: Member must have value '; the variable parts are the member path ('inferenceConfig.topP'), the bound and its direction (both from the @range trait), and the SUBMITTED VALUE rendered in the service's float formatting — scientific notation for small magnitudes, i.e. -1.0E-6 for the value -0.000001. The companion leg of the same recording (temperature=1.0+1e-6 -> 'Value '1.000001' at 'inferenceConfig.temperature' ... less than or equal to 1') already matches in the emulator, so the rule is the rendering, not the whole message.
Emulator: same code and same template, but the value is rendered as '-0.000001': actual message '1 validation error detected: Value '-0.000001' at 'inferenceConfig.topP' failed to satisfy constraint: Member must have value greater than or equal to 0'. The exact-boundary positive control (temperature=1.0, topP=0.0) is accepted and the over-max temperature leg is refused correctly, so only the float-to-string rendering diverges.
Why: the message is a requirement; a client or test that compares the refusal text byte-for-byte fails against the emulator even though the constraint was enforced. Because the renderer is shared, every @range message carrying a small- or large-magnitude float (any value AWS would print in scientific notation) is wrong in the same way.
Fix: the float-to-string rendering in the shared constraint-violation message formatter used by bedrock-runtime's modeled-input validation — format the submitted float the way AWS does (scientific notation outside the plain-decimal range) rather than expanding it.
Traces: bedrock-runtime.numeric-parameter-precision-preserved.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, 33/153 passed. Service `bedrock-runtime` is in get_bootstrap_status().bootstrapped (service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice).

Cause 1 — bedrock-runtime.converse-basic-text-roundtrip [mismatch]
  Request: Converse(modelId='amazon.nova-lite-v1:0', messages=[{'role':'user','content':[{'text':'Reply with exactly the single word PONG and nothing else.'}]}], inferenceConfig={'maxTokens':32,'temperature':0})
  Expected (AWS): the assistant text block contains 'PONG'.
  Actual: AssertionError: expected PONG in reply, got: 'No language model runs in this emulator. amazon.nova-lite-v1:0 generated no text for this request.'
Cause 1 — bedrock-runtime.converse-multiturn-context-retained [mismatch]
  Request: turn 1 states a number, turn 2 (carrying turn 1's user message and the assistant reply in `messages`) asks 'What is my favorite number? Reply with only the digits.'
  Expected (AWS): the second reply contains the number stated in turn 1.
  Actual: AssertionError: expected 7291 recalled, got: 'No language model runs in this emulator. amazon.nova-lite-v1:0 generated no text for this request.'

Cause 2 — bedrock-runtime.invoke-model-temperature-out-of-range-rejected [mismatch]
  Request: InvokeModel(modelId='amazon.nova-lite-v1:0', contentType='application/json', body={'messages':[{'role':'user','content':[{'text':'Say hello.'}]}],'inferenceConfig':{'temperature':5.0,'maxTokens':8}})
  Expected (AWS): HTTP 400, Error.Code='ValidationException', Error.Message='Malformed input request: the provided temperature must be between 0.0 and 1.0, please reformat your input and try again.'
  Actual: AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'x-amzn-bedrock-performanceconfig-latency': 'standard', 'content-length': '364', ...}}, 'contentType': 'application/json', 'performanceConfigLatency': 'standard', 'body': <StreamingBody>}

Cause 3 — bedrock-runtime.list-async-invokes-invalid-max-results-rejected [mismatch]
  The maxResults legs PASSED: maxResults=0 -> ValidationException "1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1"; maxResults=1001 -> the matching 'less than or equal to 1000' message.
  Request that failed: ListAsyncInvokes(nextToken='not-a-valid-token!!')
  Expected (AWS): Error.Code='ValidationException', Error.Message='Invalid pagination token was passed'
  Actual: AssertionError: expected message 'Invalid pagination token was passed', got 'The nextToken not-a-valid-token!! is not valid.'

Cause 4 — bedrock-runtime.numeric-parameter-precision-preserved [mismatch]
  Positive control PASSED: Converse with inferenceConfig={'temperature':1.0,'topP':0.0,'maxTokens':10} accepted; over-max leg PASSED: temperature=1.0+1e-6 -> ValidationException "1 validation error detected: Value '1.000001' at 'inferenceConfig.temperature' failed to satisfy constraint: Member must have value less than or equal to 1".
  Request that failed: Converse(modelId='amazon.nova-lite-v1:0', inferenceConfig={'temperature':0.0,'topP':-1e-6,'maxTokens':10}) with botocore parameter_validation=False so the value reaches the service.
  Expected (AWS): Error.Message="1 validation error detected: Value '-1.0E-6' at 'inferenceConfig.topP' failed to satisfy constraint: Member must have value greater than or equal to 0"
  Actual: "1 validation error detected: Value '-0.000001' at 'inferenceConfig.topP' failed to satisfy constraint: Member must have value greater than or equal to 0"

Model evidence: smithy_query query=operation Converse,ListAsyncInvokes and query=shape GuardrailConfiguration,StopReason,InferenceConfiguration on services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json — Converse POST /model/{modelId}/converse code 200, ValidationException errorKind client httpError 400; InferenceConfiguration.temperature and .topP -> Float @range {min:0,max:1}, maxTokens @range {min:1}; ListAsyncInvokes GET /async-invoke, maxResults @range {min:1,max:1000}, nextToken -> PaginationToken @length {min:1,max:2048} @pattern ^\S*$, ValidationException httpError 400.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite so the emulator is built and booted.
2. Re-replay exactly the five check ids in trace_ids.
3. Cause 1: bedrock-runtime.converse(modelId='amazon.nova-lite-v1:0', messages=[{'role':'user','content':[{'text':'Reply with exactly the single word PONG and nothing else.'}]}], inferenceConfig={'maxTokens':32,'temperature':0}) — expected 'PONG' in the text block, observed the constant notice. Then run a two-turn conversation that states a value in turn 1 and asks for it back in turn 2 — expected the value recalled, observed the same constant notice.
4. Cause 2: bedrock-runtime.invoke_model(modelId='amazon.nova-lite-v1:0', contentType='application/json', body=json({'messages':[{'role':'user','content':[{'text':'Say hello.'}]}],'inferenceConfig':{'temperature':5.0,'maxTokens':8}})) — expected 400 ValidationException 'Malformed input request: the provided temperature must be between 0.0 and 1.0, please reformat your input and try again.', observed HTTP 200. Repeat with temperature=0.5 to confirm the in-range control still succeeds.
5. Cause 3: bedrock-runtime.list_async_invokes(nextToken='not-a-valid-token!!') — expected ValidationException 'Invalid pagination token was passed', observed 'The nextToken not-a-valid-token!! is not valid.'. Also re-check maxResults=0 and maxResults=1001 (with Config(parameter_validation=False) for the 0 case) to confirm those messages stay correct.
6. Cause 4: with Config(parameter_validation=False), bedrock-runtime.converse(modelId='amazon.nova-lite-v1:0', messages=[{'role':'user','content':[{'text':'Say hello.'}]}], inferenceConfig={'temperature':0.0,'topP':-1e-6,'maxTokens':10}) — expected the refusal to render the value as '-1.0E-6', observed '-0.000001'. Confirm temperature=1.0/topP=0.0 is still accepted and temperature=1.0+1e-6 still refused with the matching message.
