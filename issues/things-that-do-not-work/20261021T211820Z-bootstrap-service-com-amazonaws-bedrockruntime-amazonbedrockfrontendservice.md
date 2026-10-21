Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the Bedrock Runtime emulator

_Filed: 2026-10-21T21:18:20Z_
_Operation: service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, holding no state. The service (restJson1, sdkId `Bedrock Runtime`, signing name `bedrock`, endpoint prefix `bedrock-runtime`) therefore has no working emulator at all.

This issue scopes the FIRST, minimal happy-path slice: synchronous inference plus the async-invoke resource lifecycle. Implement these 6 operations, in this order, each computing its response from persisted state:

1. **InvokeModel** — `POST /model/{modelId}/invoke`. Input: `modelId` (@httpLabel, required), `body` (blob @httpPayload), `contentType`/`accept` (@httpHeader Content-Type / Accept), optional `trace`, `guardrailIdentifier`, `guardrailVersion`, `performanceConfigLatency` (default `standard`), `serviceTier`, `requestMetadata`. Output: `body` (blob @httpPayload, required), `contentType` (@httpHeader Content-Type, required), echo-back of `performanceConfigLatency`/`serviceTier`. Must synthesize a deterministic, model-family-shaped JSON completion from the request body (so a client can parse it), record the invocation in state, and return `ResourceNotFoundException` (404) for a `modelId` not present in the registry of known models.
2. **Converse** — `POST /model/{modelId}/converse`. Input: `modelId` (@httpLabel, required), `messages`, `system`, `inferenceConfig`, `toolConfig`, `guardrailConfig`, `additionalModelRequestFields`. Output (all required unless noted): `output` (ConverseOutput — assistant `message` with `role=assistant` and content blocks), `stopReason` (`end_turn`, or `max_tokens` when `inferenceConfig.maxTokens` is hit), `usage` (TokenUsage: `inputTokens`, `outputTokens`, `totalTokens` — computed from the actual request/response text, not constants), `metrics` (ConverseMetrics: `latencyMs`). Must honour the conversation turn semantics: the returned assistant message must be derived from the supplied `messages`, and an invalid turn sequence (e.g. empty `messages`, or a non-alternating/leading-assistant conversation) must fail with `ValidationException` (400).
3. **CountTokens** — `POST /model/{modelId}/count-tokens`. Input: `modelId` (@httpLabel, required), `input` (CountTokensInput, required). Output: `inputTokens` (integer, required), computed by the SAME tokenizer used for `Converse.usage.inputTokens` so the two agree for identical input.
4. **StartAsyncInvoke** — `POST /async-invoke`. Input: `modelId` (required), `modelInput` (document, required), `outputDataConfig` (required), optional `clientRequestToken` (@idempotencyToken), `tags`. Output: `invocationArn` (required) matching `^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:async-invoke/[a-z0-9]{12}$`. Must create and persist an invocation record with `status=InProgress` and `submitTime=now`; the operation is `@idempotent`, so a repeat with the same `clientRequestToken` must return the SAME `invocationArn` (and a conflicting payload under a reused token must raise `ConflictException`).
5. **GetAsyncInvoke** — `GET /async-invoke/{invocationArn}`. Output: `invocationArn`, `modelArn`, `status` (`InProgress`|`Completed`|`Failed`), `submitTime`, `outputDataConfig` (all required), plus `clientRequestToken`, `lastModifiedTime`, `endTime`, `failureMessage`. Must return exactly what StartAsyncInvoke stored, resolving `modelArn` from the submitted `modelId`; an unknown-but-well-formed ARN must return `ResourceNotFoundException`/`ValidationException` per the model, never a fabricated record.
6. **ListAsyncInvokes** — `GET /async-invoke`. Input (query): `submitTimeAfter`, `submitTimeBefore`, `statusEquals`, `maxResults` (1..1000), `nextToken`, `sortBy` (default `SubmissionTime`), `sortOrder` (default `Descending`). Output: `asyncInvokeSummaries`, `nextToken`. Must list the records created by StartAsyncInvoke, apply the time/status filters, sort by submit time in the requested order, truncate at `maxResults` and emit an opaque `nextToken` that resumes exactly where the prior page ended (absent on the last page).

No canned, echoed or hardcoded responses: `GetAsyncInvoke`/`ListAsyncInvokes` must reflect records created by `StartAsyncInvoke`, token counts must be computed from the submitted text, and an invocation ARN must only resolve if it was minted by this emulator.

## State Model

A single service state struct behind the existing `crate::state::service_state(&*self.engine)` lock:

- `async_invokes: BTreeMap<String /* invocationArn */, AsyncInvokeRecord>` where `AsyncInvokeRecord { invocation_arn: String, model_id: String, model_arn: String, client_request_token: Option<String>, status: AsyncInvokeStatus, submit_time: DateTime, last_modified_time: Option<DateTime>, end_time: Option<DateTime>, failure_message: Option<String>, output_data_config: AsyncInvokeOutputDataConfig, model_input: Document, tags: Vec<Tag> }`.
- `idempotency_index: HashMap<String /* clientRequestToken */, String /* invocationArn */>` so `StartAsyncInvoke` is idempotent and can detect token reuse with a different payload.
- `models: HashMap<String /* modelId or ARN */, ModelEntry { model_id, model_arn, family, supports_converse: bool, max_output_tokens: u32 }>` — a seeded registry of recognised foundation models so `modelId` resolution and 404s are state-driven rather than always-accept.
- `invocations: Vec<InvocationLogEntry { model_id, kind: Sync|Async|Converse, input_tokens, output_tokens, at: DateTime }>` — the inference log that backs usage/metrics and lets tests observe that a call actually happened.
- `next_id: u64` / an RNG seed used to mint the 12-char lowercase-alphanumeric async-invoke id embedded in `invocationArn`.

Out of scope for this first slice (stay 501, filed separately later): `ConverseStream`, `InvokeModelWithResponseStream`, `InvokeModelWithBidirectionalStream`, `ApplyGuardrail`, `InvokeGuardrailChecks`.

## Evidence

`services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/OPERATIONS.md` lists all 11 modeled operations with their routes, required input/output members and errors — the field names and constraints quoted above (e.g. InvokeModel's `body` @httpPayload + required `contentType` output header, Converse's required `output`/`stopReason`/`usage`/`metrics`, StartAsyncInvoke's `@idempotent` + `@idempotencyToken` `clientRequestToken` and the `async-invoke/[a-z0-9]{12}` ARN pattern, ListAsyncInvokes' `@paginated(inputToken=nextToken, outputToken=nextToken, items=asyncInvokeSummaries)` and `maxResults` range 1..1000, CountTokens' required `inputTokens`). `src/ops/invoke_model.rs` lines 4 and 237 show the scaffold marker and `Err(crate::scaffolded::Scaffolded::not_implemented("InvokeModel"))`; every sibling file in `src/ops/` is the identical generated stub, so nothing in the crate holds state. No tests exist asserting any of this behaviour.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `bedrock-runtime` client at it.
2. Call CountTokens(modelId="anthropic.claude-3-sonnet-20240229-v1:0", input={invokeModel:{body:"<prompt>"}}) and note `inputTokens` > 0 (currently HTTP 501).
3. Call InvokeModel(modelId same, contentType="application/json", body=<a JSON prompt>) and expect HTTP 200 with a parseable JSON `body` payload and a `Content-Type` response header.
4. Call Converse(modelId same, messages=[{role:'user', content:[{text:'hello'}]}]) and expect `output.message.role=assistant`, `stopReason=end_turn`, and `usage.inputTokens` equal to the CountTokens result for the same text.
5. Call StartAsyncInvoke(modelId=..., modelInput={...}, outputDataConfig={s3OutputDataConfig:{s3Uri:'s3://bucket/prefix'}}, clientRequestToken='tok-1') and capture `invocationArn`.
6. Repeat the identical StartAsyncInvoke with clientRequestToken='tok-1' and expect the SAME `invocationArn` back (idempotency).
7. Call GetAsyncInvoke(invocationArn=<captured>) and expect the stored `modelArn`, `outputDataConfig`, `submitTime` and a `status` of InProgress/Completed — not a fabricated record.
8. Call ListAsyncInvokes(maxResults=1) twice, following `nextToken`, and expect the created invocation(s) listed once each in Descending submit-time order with no duplicates and no `nextToken` on the final page.
9. Call GetAsyncInvoke with a well-formed but never-minted ARN and expect a modeled 404/400 error, not a synthesized success.
