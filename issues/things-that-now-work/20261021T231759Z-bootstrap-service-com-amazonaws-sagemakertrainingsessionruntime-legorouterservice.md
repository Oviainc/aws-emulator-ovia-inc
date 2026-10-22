Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the SageMakerTrainingSessionRuntime (LegoRouterService) emulator

_Filed: 2026-10-21T23:17:59Z_
_Operation: service-com-amazonaws-sagemakertrainingsessionruntime-legorouterservice | Class: stub_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 6 operations in `src/ops/` is an untouched scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST stateful slice covering the service's core submit-then-poll loop (restJson1, sigv4 signing name `sagemaker`, endpoint prefix `training-session-runtime.sagemaker`).

### Happy path to implement (in order)

1. **ExecuteTrainingOperation** — `POST /v1/training-sessions/{trainingSessionName}/operations`. Input: `trainingSessionName` (@httpLabel, 1..64, `^[a-zA-Z0-9-]+$`, required), `sequenceNumber` (integer, range 1.., required), `clientToken` (1..128, optional), `operationConfig` (union `OperationConfig`: exactly one of `forward` | `forwardBackward` | `optimizerStep` | `saveWeights` | `saveState` | `loadState`, required). Must MINT and PERSIST a new `operationId` (1..64, `^[a-zA-Z0-9-]+$`) against the named session, store the submitted config and the sequence number, and return that id. `clientToken` must be idempotent: a repeat call with the same token for the same session returns the SAME `operationId` without creating a second operation. A duplicate/out-of-order `sequenceNumber` for the same session is `ConflictException` (409).
2. **GetTrainingOperationResult** — `GET /v1/training-sessions/{trainingSessionName}/operations/{operationId}`. Must look the operation up in stored state and return `status` (`RequestStatus`: `Pending` | `InProgress` | `Completed` | `Failed` | `Skipped`, required), plus `operationResult` (union `OperationResult`, member matching the submitted `operationConfig` member — e.g. `forwardBackward` submitted ⇒ `forwardBackward` populated with `lossFnOutputs`/`metrics`/`tokenUsage`), `errorDetails` when failed, and `tokenUsage` (`TrainingTokenUsage{trainingTokens}`). An unknown `operationId` or session is `ResourceNotFoundException` (404) — NOT a canned `Completed`.
3. **ExecuteSamplingOperation** — `POST /v1/training-sessions/{trainingSessionName}/sampling-operations`. Input: `trainingSessionName` (@httpLabel, required), `operationConfig` (union `SampleOperationConfig`: exactly one of `sample` | `computeLogprobs` | `loadWeightsForSampling`, required). Must mint and persist an `operationId` in the session's sampling-operation namespace (separate from training operations) and return it.
4. **GetSamplingOperationResult** — `GET /v1/training-sessions/{trainingSessionName}/sampling-operations/{operationId}`. Must return the stored `status` and an `operationResult` (`SampleOperationResult`) whose populated member matches what was submitted — for `sample`, a `SampleResponse{sequences, promptLogprobs, topkPromptLogprobs}` with `sequences` length equal to the submitted `SampleConfig.numSamples`, `promptLogprobs` present only when `includePromptLogprobs` is true, and `topkPromptLogprobs` present only when `topkPromptLogprobs > 0`. Unknown id ⇒ 404.
5. **SampleTrainingSession** — `POST /training-sessions/{trainingSessionName}/sample`. Synchronous variant: reads `body` (@httpPayload blob), `Content-Type`, `Accept`, `X-Amzn-SageMaker-Model-Package` headers; must return a `body` blob (required) and a `contentType` header derived from the session's stored state/checkpoint, not a blind echo of the request body.

Because this runtime service has no session-create operation of its own (sessions are created by the control plane), the emulator should materialise a session record on first use of a `trainingSessionName` (or expose a test seam to pre-seed sessions) so the loop above is reachable; a never-seen session on a GET must still be 404.

### Non-goals for this slice
`SampleTrainingSessionWithResponseStream` stays stubbed (501) and will be filed as its own gap issue.

### Requirements
- Every response must be COMPUTED FROM PERSISTED STATE reached via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoed inputs, no random id that is never stored. A submit followed by a get must return exactly what was submitted.
- Enforce the modeled constraints on the happy-path inputs: label length 1..64 and pattern `^[a-zA-Z0-9-]+$`, `sequenceNumber >= 1`, `SampleConfig.numSamples` in 1..1000, exactly one union member set — violations are `ValidationException` (400).
- Status progression must be observable: an operation may start `Pending`/`InProgress` and become `Completed` on a later poll, but the transition must come from stored state (e.g. a stored completion deadline/poll count), not a coin flip.
- Done = an integration test that submits a `forwardBackward` training operation, polls it to `Completed` with matching `operationResult` and `tokenUsage`, submits a `sample` sampling operation, polls it to a `SampleResponse` with `numSamples` sequences, and calls `SampleTrainingSession` — all with correct bodies, not merely 2xx.

## State Model

- `TrainingSessionRuntimeState { sessions: Map<String /*trainingSessionName*/, Session> }` — the single struct stored in the shared engine.
- `Session { name: String, created_at: Timestamp, next_sequence_number: i32, training_ops: Map<String /*operationId*/, TrainingOp>, sampling_ops: Map<String /*operationId*/, SamplingOp>, client_tokens: Map<String /*clientToken*/, String /*operationId*/>, checkpoints: Vec<Checkpoint>, latest_model_package_arn: Option<String> }`
- `TrainingOp { operation_id: String, sequence_number: i32, client_token: Option<String>, config: OperationConfig, status: RequestStatus, result: Option<OperationResult>, error_details: Option<String>, token_usage: Option<TrainingTokenUsage>, submitted_at: Timestamp }`
- `SamplingOp { operation_id: String, config: SampleOperationConfig, status: RequestStatus, result: Option<SampleOperationResult>, error_details: Option<String>, submitted_at: Timestamp }`
- `Checkpoint { id: String, created_by_operation_id: String, created_at: Timestamp }` — written by `saveWeights`/`saveState` operations and read by `loadState`/`loadWeightsForSampling` and by `SampleTrainingSession`.

## Evidence

OPERATIONS.md for this crate lists all 6 operations with routes, members and errors: ExecuteTrainingOperation (`POST /v1/training-sessions/{trainingSessionName}/operations`, required `sequenceNumber`/`operationConfig`, optional `clientToken`, output required `operationId`), GetTrainingOperationResult (`GET .../operations/{operationId}`, @readonly, output `status`/`operationResult`/`errorDetails`/`tokenUsage`), ExecuteSamplingOperation and GetSamplingOperationResult (the `/sampling-operations` pair), SampleTrainingSession and SampleTrainingSessionWithResponseStream (blob @httpPayload in/out). `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' src/ops/` matches all six handler files (execute_training_operation.rs, get_training_operation_result.rs, execute_sampling_operation.rs, get_sampling_operation_result.rs, sample_training_session.rs, sample_training_session_with_response_stream.rs), i.e. the whole service answers 501 and persists nothing. Shape details from the Smithy model: `OperationConfig`/`OperationResult` and `SampleOperationConfig`/`SampleOperationResult` are unions with matching member names ('Exactly one member must be set'), `SampleConfig` requires `prompt`/`numSamples`/`samplingParams` with `numSamples` range 1..1000 and defaults `includePromptLogprobs=false`/`topkPromptLogprobs=0`, `SampleResponse` requires `sequences`, and `TrainingTokenUsage` carries `trainingTokens`. All ops declare ConflictException(409), ResourceNotFoundException(404), ValidationException(400), ThrottlingException(429), InternalServerException(500).

## Steps to Reproduce

1. Start the emulator and target the SageMakerTrainingSessionRuntime endpoint (restJson1, signing name `sagemaker`).
2. POST /v1/training-sessions/demo-session/operations with {"sequenceNumber":1,"clientToken":"tok-1","operationConfig":{"forwardBackward":{"lossFnName":...,"datumList":[...]}}} — expect 200 with a fresh `operationId`; today: HTTP 501 NotImplemented.
3. Repeat the identical POST with clientToken `tok-1` — expect the SAME `operationId` (idempotency), no new operation.
4. GET /v1/training-sessions/demo-session/operations/{operationId} — expect `status` from stored state, eventually `Completed` with `operationResult.forwardBackward` (metrics/lossFnOutputs) and `tokenUsage.trainingTokens`.
5. POST /v1/training-sessions/demo-session/sampling-operations with {"operationConfig":{"sample":{"prompt":...,"numSamples":3,"samplingParams":{...}}}} — expect 200 with an `operationId`.
6. GET /v1/training-sessions/demo-session/sampling-operations/{operationId} — expect `operationResult.sample.sequences` with exactly 3 entries.
7. POST /training-sessions/demo-session/sample with a payload body — expect 200 with a non-empty `body` blob and a `Content-Type` response header.
8. GET /v1/training-sessions/demo-session/operations/does-not-exist — expect ResourceNotFoundException (404).
9. POST /v1/training-sessions/demo-session/operations with sequenceNumber 0 or an empty operationConfig union — expect ValidationException (400).
