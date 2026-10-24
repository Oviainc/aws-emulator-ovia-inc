Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-runtime.bedrock-runtime-prompt-cache-point-reuses-input-tokens-across-calls
Issue-Shape-Id: com.amazonaws.bedrockruntime#AmazonBedrockFrontendService
Issue-Key: bedrock-runtime.bedrock-runtime-prompt-cache-point-reuses-input-tokens-across-calls|run#6|AssertionError|assert cold is not None, (
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# Converse accepts a cachePoint content block and ignores it: TokenUsage never reports cacheWriteInputTokens/cacheReadInputTokens

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Converse

## Description
ONE CAUSE on the Converse prompt-caching surface. It is independent of what text a model generates: every value asserted is token accounting the service itself computes, not generated content.

Cause 1 of 1 - a `cachePoint` content block is accepted but has no effect, and the prompt-cache token counters are never emitted.
AWS: Smithy (services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json) models ContentBlock as a union with member `cachePoint` -> CachePointBlock {type: CachePointType (required, values [default]), ttl: CacheTTL}, documented as "CachePoint to include in the message" / "Defines a section of content to be cached for reuse in subsequent API calls". ConverseResponse.usage -> TokenUsage carries, besides the required inputTokens/outputTokens/totalTokens, the optional members `cacheWriteInputTokens` ("The number of input tokens written to the cache for the request"), `cacheReadInputTokens` ("The number of input tokens read from the cache for the request") and `cacheDetails`. Recorded behavior (the check's measurement, 6 back-to-back trials): when a cacheable prefix precedes a cachePoint block, the first call reports a non-zero cacheWriteInputTokens, a byte-identical replay reports cacheReadInputTokens equal to what was written and cacheWriteInputTokens 0, inputTokens then counts only the content AFTER the cachePoint (so it is small and identical on both calls), totalTokens = inputTokens + outputTokens + cacheRead + cacheWrite, and a prefix diverging in its first token does not hit. Caching is best-effort (1 of 6 recorded attempts did not cache), so the requirement graded here is that it happens at all over repeated attempts, not on any single call.
Emulator: over 5 consecutive attempts, each with a fresh ~1200-section prefix followed by {"cachePoint": {"type": "default"}}, every response carries usage {'inputTokens': 34800, 'outputTokens': 16, 'totalTokens': 34816} - cacheWriteInputTokens and cacheReadInputTokens are absent/zero on every call and the entire prefix is billed as ordinary inputTokens. The cachePoint block is accepted (200, no ValidationException) and silently discarded; no server-side prefix state is retained for the next call.
Why: a client that instruments or budgets prompt caching cannot observe it at all. Code that reads usage.cacheReadInputTokens / cacheWriteInputTokens to compute cost or cache-hit rate sees permanent zeros, dashboards and regression tests built on them pass against the emulator and change behavior against AWS, and the characteristic savings of a cached prefix (inputTokens collapsing to just the post-cachePoint suffix) is never reproduced, so any test of a caching code path is vacuous.
Fix: the Converse/ConverseStream handler's content-block handling plus its token-accounting/state stage - honour the cachePoint block by retaining the prefix before it in service state keyed on that prefix, attribute cached prefix tokens to cacheWriteInputTokens on the cold call and cacheReadInputTokens on a matching replay (keeping totalTokens closed over all four counters and inputTokens counting only the content after the cachePoint), and miss when the prefix diverges in its first token.
Traces: bedrock-runtime.bedrock-runtime-prompt-cache-point-reuses-input-tokens-across-calls.

Related but NOT the same cause: the open issue 20261023T232022Z-bedrock-runtime-synchronous-inference-guardrailconfig-and-toolconfig-silently.md covers seven other Converse/InvokeModel causes (guardrailConfig and toolConfig ignored, no authorization gate, single-frame response stream, not-found class, empty system block, embedding path). None of its causes touches cachePoint or the TokenUsage cache counters.

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1, bedrock-runtime 11/63 passed. The emulator served every one of these calls with HTTP 200, so the service is up and answering wrongly.

bedrock-runtime.bedrock-runtime-prompt-cache-point-reuses-input-tokens-across-calls [mismatch]:
  AssertionError: the service never cached a prefix in 5 attempts, each over a 1200-section prefix well above this model's minimum. Real AWS missed 1 of 6 measured attempts, so 5 consecutive misses means cachePoint is being accepted and ignored; per-attempt usage was [{'inputTokens': 34800, 'outputTokens': 16, 'totalTokens': 34816}, {'inputTokens': 34800, 'outputTokens': 16, 'totalTokens': 34816}, {'inputTokens': 34800, 'outputTokens': 16, 'totalTokens': 34816}, {'inputTokens': 34800, 'outputTokens': 16, 'totalTokens': 34816}, {'inputTokens': 34800, 'outputTokens': 16, 'totalTokens': 34816}]

Expected (AWS, recorded): at least one of the attempts reports usage.cacheWriteInputTokens > 0; its byte-identical replay reports cacheReadInputTokens == that value and cacheWriteInputTokens == 0; inputTokens is identical and small on both calls; a prefix prefixed with an extra leading token reports cacheReadInputTokens == 0.
Actual (emulator): no attempt ever reports either cache counter; the full prefix is charged as inputTokens on every call, so no replay can be distinguished from a cold call.

Model evidence: smithy_query query=shape shape=TokenUsage,CachePointBlock,ContentBlock,CachePointType on services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json - ContentBlock union member `cachePoint` -> CachePointBlock{type: CachePointType[default] @required, ttl: CacheTTL}; TokenUsage members cacheReadInputTokens / cacheWriteInputTokens / cacheDetails (all @range min 0) alongside the required inputTokens/outputTokens/totalTokens.

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Re-replay the check id in trace_ids: bedrock-runtime.bedrock-runtime-prompt-cache-point-reuses-input-tokens-across-calls.
3. Manually: call Converse on a text model with messages=[{role: user, content: [{text: <long unique prefix, ~1200 sentences / tens of thousands of tokens>}, {cachePoint: {type: 'default'}}, {text: 'Reply with the single word ALPHA.'}]}] and inferenceConfig={'maxTokens': 16, 'temperature': 0.0}. Expected: the response's usage reports a non-zero cacheWriteInputTokens (best-effort, so retry a few times with a fresh prefix each time); observed: usage has only inputTokens/outputTokens/totalTokens with the whole prefix in inputTokens.
4. Repeat the identical call with the SAME prefix. Expected: usage.cacheReadInputTokens equals the previously written count, usage.cacheWriteInputTokens is 0, and inputTokens is unchanged and small (only the post-cachePoint suffix); observed: identical usage to the first call, both cache counters absent.
5. Repeat with 'PREAMBLE ' prepended to the prefix. Expected: cacheReadInputTokens == 0 (a first-token divergence must miss); observed: no counters at all, so the miss is unobservable.
6. On every call check that totalTokens == inputTokens + outputTokens + cacheReadInputTokens + cacheWriteInputTokens.
