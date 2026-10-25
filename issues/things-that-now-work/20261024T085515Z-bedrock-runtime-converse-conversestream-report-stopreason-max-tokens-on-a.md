Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-runtime__cloudwatch.converse-invocations-reach-cloudwatch-with-the-documented-unit-and-dimension bedrock-runtime__cloudwatch.refused-invocation-counted-as-client-error-never-as-an-invocation bedrock__bedrock-runtime.bedrock-runtime-guardrail-blocks-the-model-output-after-generation
Issue-Key: bedrock-runtime__cloudwatch.converse-invocations-reach-cloudwatch-with-the-documented-unit-and-dimension|run#8|AssertionError|assert resp["stopReason"] == "end_turn", (
Issue-Key: bedrock-runtime__cloudwatch.refused-invocation-counted-as-client-error-never-as-an-invocation|run#10|AssertionError|assert ok["stopReason"] == "end_turn" and "PONG" in _text(ok).upper(), (
Issue-Key: bedrock__bedrock-runtime.bedrock-runtime-guardrail-blocks-the-model-output-after-generation|run#11|AssertionError|assert allowed["stopReason"] == "end_turn", (
Issue-Commit: 1f773e23b3e0bd2b9ce8ba90d57212de869e61a4

# Converse/ConverseStream report stopReason='max_tokens' on a normal short turn: the synthesized completion ignores inferenceConfig.maxTokens and is truncated instead of finishing

> _Note: this issue was identified against the code base at commit `1f773e23b3e0bd2b9ce8ba90d57212de869e61a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Converse

## Description
ONE CAUSE on the synchronous/streaming Converse surface of bedrock-runtime. It is an assertion on the response ENVELOPE (the required `stopReason` member), not on generated model text: the three checks below all fail on the value of `stopReason`, before any content comparison.

Related already-open issues, named so a fixer sees the overlap and does not re-file: `20261023T232022Z-bedrock-runtime-synchronous-inference-guardrailconfig-and-toolconfig-silently.md` mentions `stopReason='max_tokens'` ONLY in its Cause 2, for the forced-tool-use request whose token budget cannot complete the tool call (expected 424 ModelErrorException). The cause below is different and is not covered there: it is a plain Converse turn with no toolConfig at all, where AWS finishes normally with `end_turn`. `20261024T025951Z-bedrock-runtime-converse-accepts-a-cachepoint-content-block-and-ignores.md` covers TokenUsage cache members only.

Cause 1 of 1 — a short, fully-answerable Converse turn with a modest inferenceConfig.maxTokens is reported as truncated.
AWS: Smithy Converse -> POST /model/{modelId}/converse, 200; ConverseResponse members `output`, `stopReason`, `usage`, `metrics` are all @required, and `stopReason` targets the StopReason enum with values [end_turn, tool_use, max_tokens, stop_sequence, guardrail_intervened, content_filtered, malformed_model_output, malformed_tool_use, model_context_window_exceeded]. InferenceConfiguration.maxTokens is documented as "The maximum number of tokens to allow in the generated response" (@range min 1) — a CEILING, not a target. Recorded behavior only for the class choice: when the model completes its answer within that ceiling, the turn ends with stopReason='end_turn'; 'max_tokens' is reserved for a completion that actually hit the ceiling. The failing checks use prompts whose correct answer is a single short token (e.g. "Reply with only the lowercase English word for the result of 2+3") together with inferenceConfig such as {"maxTokens": 20, "temperature": 0.0}, and AWS answers them with 'end_turn' (Smithy model read: services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json, query=operation Converse and query=shape StopReason,InferenceConfiguration).
Emulator: the handler substitutes a FIXED-LENGTH synthesized completion for every request (example text: 'No language model runs in this Amazon Bedrock Runtime emulator, so <modelId> generated no text for this request. The request carried N message(s) and M input token(s).'), then truncates that fixed text at the caller's maxTokens and reports stopReason='max_tokens'. Observed truncated payloads: '...so amazon.nova-lite-v1:0 generated no' and '...so amazon' for maxTokens=20 and smaller. Because the synthesized text is a constant whose length is unrelated to the request, EVERY request with a small-but-legitimate maxTokens is reported as truncated, and the happiest path of the service's primary operation never returns 'end_turn'.
Why: `stopReason` is the one member every Converse client branches on. A client that treats 'max_tokens' as "the answer is incomplete" will retry, continue-generate, raise, or mark the turn unusable — against the emulator that happens on every normal turn, and against AWS it never happens for the same request. It also makes it impossible to exercise any downstream contract that is gated on a successful turn (the two cloudwatch checks below never get to assert the AWS/Bedrock `Invocations` vs `InvocationClientErrors` classification, and the output-side guardrail check never gets to assert `trace.guardrail.outputAssessments`), so this one envelope field hides several further contracts behind it.
Fix: the Converse/ConverseStream handler's completion-synthesis and stop-reason stage in services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/src/ops/ (the same stage that fills ConverseResponse.output/usage/metrics): the synthesized completion must fit the requested maxTokens budget and report stopReason='end_turn' when it was not truncated, reserving 'max_tokens' for a completion that genuinely reached the ceiling. The same framing is shared by ConverseStream's messageStop frame.
Traces: bedrock-runtime__cloudwatch.converse-invocations-reach-cloudwatch-with-the-documented-unit-and-dimension, bedrock-runtime__cloudwatch.refused-invocation-counted-as-client-error-never-as-an-invocation, bedrock__bedrock-runtime.bedrock-runtime-guardrail-blocks-the-model-output-after-generation.

Note on service attribution: the three grounding checks carry the composite leading tokens `bedrock-runtime__cloudwatch` and `bedrock__bedrock-runtime` (multi-service checks). The diverging response is bedrock-runtime's Converse response, so this issue is filed under service `bedrock-runtime`; `bedrock`, `bedrock-runtime` and `cloudwatch` are all bootstrapped, so the gate verdict is FILE for every constituent.

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1. bedrock-runtime 11/63 passed and bedrock 2/18 passed, so the emulator is up and answering these calls with 200s — it is answering wrongly, not refusing.

1) bedrock-runtime__cloudwatch.converse-invocations-reach-cloudwatch-with-the-documented-unit-and-dimension
   check_bedrock_runtime__cloudwatch_converse_invocations_reach_cloudwatch_with_the_documented_unit_and_dimension.py line 151:
     assert resp["stopReason"] == "end_turn", (...)
   AssertionError: invocation 0 must succeed for it to be counted as an Invocation at all; got stopReason='max_tokens'
   Expected: 200 with stopReason='end_turn'. Observed: 200 with stopReason='max_tokens'.

2) bedrock-runtime__cloudwatch.refused-invocation-counted-as-client-error-never-as-an-invocation
   check_bedrock_runtime__cloudwatch_refused_invocation_counted_as_client_error_never_as_an_invocation.py line 202:
     assert ok["stopReason"] == "end_turn" and "PONG" in _text(ok).upper(), (...)
   AssertionError: ... stopReason='max_tokens' text='No language model runs in this Amazon Bedrock Runtime emulator, so amazon.nova-lite-v1:0 generated no'
   The text half of that assertion is generated model content and is NOT the requirement filed here; the stopReason half is. Note the text is the fixed placeholder cut off mid-sentence, which is what produced the 'max_tokens' classification.

3) bedrock__bedrock-runtime.bedrock-runtime-guardrail-blocks-the-model-output-after-generation
   check_bedrock__bedrock_runtime_bedrock_runtime_guardrail_blocks_the_model_output_after_generation.py line 144:
     assert allowed["stopReason"] == "end_turn", (...)
   AssertionError: an answer that does not contain the blocked word must pass; got 'max_tokens' text='No language model runs in this Amazon Bedrock Runtime emulator, so amazon'
   Request: Converse(modelId='amazon.nova-lite-v1:0', messages=[{'role':'user','content':[{'text':'Reply with only the lowercase English word for the result of 2+3. No punctuation, no other words.'}]}], inferenceConfig={'maxTokens': 20, 'temperature': 0.0}, guardrailConfig={...,'trace':'enabled'}). Expected stopReason='end_turn' (the one-word answer is far inside a 20-token ceiling); observed 'max_tokens' with the placeholder truncated after 'so amazon'.

Model evidence: smithy_query query=operation operation=Converse and query=shape shape=StopReason,InferenceConfiguration on services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json — ConverseResponse.stopReason @required -> StopReason enum (includes end_turn and max_tokens); InferenceConfiguration.maxTokens @range min 1, documented as the maximum number of tokens to ALLOW in the generated response.

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Re-replay the check ids in trace_ids.
3. Call Converse(modelId='amazon.nova-lite-v1:0', messages=[{'role':'user','content':[{'text':'Reply with only the lowercase English word for the result of 2+3. No punctuation, no other words.'}]}], inferenceConfig={'maxTokens': 20, 'temperature': 0.0}). Expected HTTP 200 with stopReason='end_turn'; observed HTTP 200 with stopReason='max_tokens' and the response text being the emulator's fixed placeholder cut off mid-sentence.
4. Repeat with inferenceConfig omitted entirely and then with a large maxTokens (e.g. 512): stopReason must be 'end_turn' in those cases too, and must only become 'max_tokens' when the completion actually reaches the supplied ceiling.
5. Repeat step 3 through ConverseStream and read the messageStop frame: its stopReason must agree with the unary call ('end_turn'), not 'max_tokens'.
6. With the fix in place, the two bedrock-runtime__cloudwatch checks can proceed past their barrier to assert the AWS/Bedrock Invocations / InvocationClientErrors classification, and bedrock__bedrock-runtime.bedrock-runtime-guardrail-blocks-the-model-output-after-generation can proceed to assert trace.guardrail.outputAssessments.
