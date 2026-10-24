Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the SageMaker Runtime emulator

_Filed: 2026-10-21T21:28:13Z_
_Operation: service-com-amazonaws-sagemakerruntime-amazonsagemakerruntime | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in this crate is an unmodified scaffold: `src/ops/invoke_endpoint.rs` and `src/ops/invoke_endpoint_async.rs` both carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and `InvokeEndpointWithResponseStream` is listed in `src/ops/SKIPPED_STUBS.md` as modeled-but-unwired. `src/state.rs` holds `// TODO: add service-specific state fields`, so the service holds zero state.

Implement this FIRST SLICE, in order. SageMaker Runtime is a data-plane-only service (there is no CreateEndpoint here — no `services/*sagemaker*` control-plane crate exists in the repo), so the "create" step of the lifecycle is an emulator-local endpoint registry that must be consultable, plus a deterministic in-emulator "model" that produces the inference response. No step may return a canned, zeroed or blindly echoed response: each response field must be computed from the persisted state described below.

1. **InvokeEndpoint** (`POST /endpoints/{EndpointName}/invocations` -> 200).
   - Inputs to honour: `EndpointName` (@httpLabel, required), `Body` (@httpPayload blob, required, len 0..6291456), `ContentType`/`Accept` headers, `CustomAttributes` (`X-Amzn-SageMaker-Custom-Attributes`), `TargetModel`, `TargetVariant`, `TargetContainerHostname`, `InferenceId`, `InferenceComponentName`, `SessionId` (`NEW_SESSION` or an existing id).
   - Must look `EndpointName` up in the endpoint registry. Unknown endpoint => `ValidationError` (HTTP 400), NOT a hollow 200. An endpoint registered as not-yet-InService / model-not-loaded => `ModelNotReadyException` (HTTP 429); a registered endpoint whose configured model is set to fail => `ModelError` (HTTP 424) with its `OriginalStatusCode`/`OriginalMessage`.
   - Outputs must be derived, not fixed: `Body` from the registered model behaviour over the request `Body` (an echo/identity model is acceptable as the default, but it must read the stored per-endpoint model config), `ContentType` from `Accept` (or the endpoint's default), `InvokedProductionVariant` = `TargetVariant` when supplied else a variant chosen from the endpoint's stored variant list/weights, `CustomAttributes` forwarded verbatim only when the stored model config says to echo it.
   - `SessionId: NEW_SESSION` must create a session in state and return `NewSessionId` in the modeled form `<id>; Expires=<ISO8601Z>`; a subsequent call with that id must find the live session (expired/unknown session id => `ValidationError` 400); a close request must remove it from state and return `ClosedSessionId`.
   - Every invocation must append an invocation record (keyed by effective `InferenceId`) so a later read/assertion can observe it.
2. **InvokeEndpointAsync** (`POST /endpoints/{EndpointName}/async-invocations` -> 202).
   - Inputs: `EndpointName` (required), `InputLocation` (`^(https|s3)://...`), `ContentType`/`Accept` (`X-Amzn-SageMaker-Content-Type`/`-Accept`), `CustomAttributes`, `InferenceId`, `S3OutputPathExtension`, `Filename`, `RequestTTLSeconds` (60..21600), `InvocationTimeoutSeconds` (1..3600), optional `Body`.
   - Unknown endpoint, or an endpoint registered without an async-inference output path => `ValidationError` 400.
   - Must persist an async-invocation record and return HTTP 202 with `InferenceId` (the caller's value, or a generated UUID stored in state), `OutputLocation` computed from the endpoint's stored `S3OutputPath` + `S3OutputPathExtension`/`Filename` + inference id, and `FailureLocation` computed from the endpoint's stored failure path. These three must be stable: re-reading the record must yield the same strings.
3. **InvokeEndpointWithResponseStream** (`POST /endpoints/{EndpointName}/invocations-response-stream` -> 200) — hand-author `src/ops/invoke_endpoint_with_response_stream.rs` (the generator declined it: `@streaming` event-stream output). It must reuse the same registry + model path as InvokeEndpoint and emit the `ResponseStream` as one or more `PayloadPart` events framed as `vnd.amazon.eventstream`, with `InvokedProductionVariant`/`ContentType`/`CustomAttributes` headers computed the same way, and `ModelStreamError`/`InternalStreamFailure` reachable for the failure-configured endpoint. If event-stream framing is not yet available in the runtime, this step may land immediately after steps 1-2 — but steps 1 and 2 are not optional.

Also needed for the slice to be usable: a way to register endpoints in state (e.g. an admin/seed hook or lazily auto-registering an endpoint on first invoke with echo-model defaults). Whichever is chosen, invoking a name that was never registered must still produce `ValidationError` 400 so the unknown-endpoint path is observable.

## State Model

In `ComAmazonawsSagemakerruntimeAmazonsagemakerruntimeState` (`src/state.rs`):

- `endpoints: HashMap<String, Endpoint>` keyed by `EndpointName`.
  - `Endpoint { name: String, status: EndpointStatus /* InService | Creating | Failed */, variants: Vec<Variant>, default_content_type: Option<String>, model: ModelBehaviour, async_config: Option<AsyncConfig>, inference_components: HashSet<String>, target_models: HashSet<String>, created_at: SystemTime }`
  - `Variant { name: String, weight: f32 }` — drives `InvokedProductionVariant` when `TargetVariant` is absent.
  - `ModelBehaviour { kind: Echo | FixedBody(Vec<u8>) | Fail { original_status_code: i32, message: String } | NotReady, echo_custom_attributes: bool, response_content_type: Option<String> }`
  - `AsyncConfig { s3_output_path: String, s3_failure_path: String }`
- `sessions: HashMap<String, Session>` keyed by session id.
  - `Session { id: String, endpoint_name: String, expires_at: SystemTime, created_at: SystemTime, request_count: u64 }`
- `invocations: Vec<InvocationRecord>` (plus `by_inference_id: HashMap<String, usize>`).
  - `InvocationRecord { inference_id: String, endpoint_name: String, request_body: Vec<u8>, content_type: Option<String>, accept: Option<String>, custom_attributes: Option<String>, target_model: Option<String>, target_variant: Option<String>, target_container_hostname: Option<String>, inference_component_name: Option<String>, session_id: Option<String>, invoked_variant: String, response_body: Vec<u8>, at: SystemTime }`
- `async_invocations: HashMap<String, AsyncInvocation>` keyed by inference id.
  - `AsyncInvocation { inference_id: String, endpoint_name: String, input_location: Option<String>, inline_body: Option<Vec<u8>>, output_location: String, failure_location: String, request_ttl_seconds: Option<i32>, invocation_timeout_seconds: Option<i32>, custom_attributes: Option<String>, content_type: Option<String>, accept: Option<String>, status: AsyncStatus /* InProgress | Completed | Failed */, submitted_at: SystemTime }`

Operations outside this slice stay 501 and are filed separately later.

## Evidence

OPERATIONS.md (crate root) lists all 3 operations with routes, members and errors: `InvokeEndpoint` `POST /endpoints/{EndpointName}/invocations -> 200` with required `EndpointName`/`Body`, header-bound `SessionId`/`TargetVariant`/`CustomAttributes`, outputs `Body`/`ContentType`/`InvokedProductionVariant`/`CustomAttributes`/`NewSessionId`/`ClosedSessionId`, errors `ValidationError (400)`, `ModelError (424)`, `ModelNotReadyException (429)`, `ServiceUnavailable (503)`, `InternalFailure (500)`, `InternalDependencyException (530)`; `InvokeEndpointAsync` `POST /endpoints/{EndpointName}/async-invocations -> 202` with `InputLocation`/`RequestTTLSeconds (60..21600)`/`InvocationTimeoutSeconds (1..3600)` and outputs `InferenceId`/`OutputLocation`/`FailureLocation`; `InvokeEndpointWithResponseStream` with a `ResponseStream` `@streaming` payload. Protocol per OPERATIONS.md: restJson1, sigv4 signing name `sagemaker`, endpoint prefix `runtime.sagemaker`. Stub proof: `src/ops/invoke_endpoint.rs:4` and `src/ops/invoke_endpoint_async.rs` carry `//! SCAFFOLDED, NOT IMPLEMENTED:` and `invoke_endpoint.rs:258` returns `Err(crate::scaffolded::Scaffolded::not_implemented("InvokeEndpoint"))`; `src/ops/SKIPPED_STUBS.md:21` records InvokeEndpointWithResponseStream as unwired; `src/state.rs:8` is an empty `// TODO: add service-specific state fields` struct, so no state exists. No tests beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Register (or seed) an endpoint named `my-endpoint` in the emulator with an echo model, one production variant `AllTraffic`, and an async output path `s3://bucket/out/`.
2. `POST /endpoints/my-endpoint/invocations` with `Content-Type: application/json`, body `{"instances":[1,2,3]}` and `X-Amzn-SageMaker-Custom-Attributes: trace=abc` — expect HTTP 200, a body derived from the request by the stored model, `x-Amzn-Invoked-Production-Variant: AllTraffic`, and the custom attribute echoed per the stored model config (currently: HTTP 501).
3. `POST /endpoints/does-not-exist/invocations` — expect HTTP 400 `ValidationError`, not 200 and not 501.
4. `POST /endpoints/my-endpoint/invocations` with `X-Amzn-SageMaker-Session-Id: NEW_SESSION` — expect 200 with `X-Amzn-SageMaker-New-Session-Id: <id>; Expires=<ISO8601Z>`; repeat the call with that `<id>` and expect it to be accepted (same session, request_count incremented), then close it and expect `X-Amzn-SageMaker-Closed-Session-Id: <id>` and a 400 `ValidationError` on a third call with the closed id.
5. `POST /endpoints/my-endpoint/async-invocations` with `X-Amzn-SageMaker-InputLocation: s3://bucket/in/req.json` and `X-Amzn-SageMaker-Inference-Id: req-1` — expect HTTP 202 with `InferenceId: req-1` in the JSON body and `X-Amzn-SageMaker-OutputLocation`/`X-Amzn-SageMaker-FailureLocation` derived from the endpoint's stored S3 paths (currently: HTTP 501).
6. Repeat the async call with the same `InferenceId` and confirm the same `OutputLocation` comes back from the persisted record (no random per-request value).
7. `POST /endpoints/my-endpoint/async-invocations` with `X-Amzn-SageMaker-RequestTTLSeconds: 10` — expect HTTP 400 (below the modeled range 60..21600).
8. Register `failing-endpoint` with a failing model and invoke it — expect HTTP 424 `ModelError`; register `provisioning-endpoint` as not ready and invoke it — expect HTTP 429 `ModelNotReadyException`.
9. `POST /endpoints/my-endpoint/invocations-response-stream` — expect HTTP 200 with an `application/vnd.amazon.eventstream` body carrying `PayloadPart` events computed by the same stored model (currently unroutable/501).

## Status note (left by the fixer of this issue)

All three operations are implemented in
`services/service-com-amazonaws-sagemakerruntime-amazonsagemakerruntime`, but NOT with the
emulator-local endpoint registry this issue sketched: by the time the slice landed, the
SageMaker CONTROL PLANE crate (`service-com-amazonaws-sagemaker-sagemaker`) existed and
owned endpoints, so the premise above ("no `services/*sagemaker*` control-plane crate
exists") no longer held. Every invocation therefore resolves the endpoint it names in the
control plane's own store through the one shared engine — `crate::state::resolve_endpoint`,
the single resolver all three operations ask — and this crate keeps no endpoint of its
own. What its state holds is what the data plane produced: the bounded log of answered
invocations and the bounded queue of accepted asynchronous submissions.

- `InvokeEndpoint` refuses an unknown endpoint, one with no deployment behind it, a
  `TargetVariant` the endpoint does not host, `EnableExplanations` without an
  `ExplainerConfig` and a `SessionId` naming no session (all 400 `ValidationError`), and
  answers an otherwise valid invocation with the modelled 424 `ModelError`.
- `InvokeEndpointAsync` answers 202 with `InferenceId` and the `OutputLocation` /
  `FailureLocation` computed from the endpoint's own `AsyncInferenceConfig$OutputConfig`
  paths plus the request's `S3OutputPathExtension` / `Filename`, and queues the submission.
- `InvokeEndpointWithResponseStream` is hand-authored and routed through
  `Service::override_operation` (the generator declines a `@streaming` output).

What this emulator will NOT do, and why: a SageMaker model is customer code in a container
image, which a generated emulator does not run. So no `Body` is synthesised for a
real-time invocation, no `PayloadPart` is framed for the streaming route (a part carries
the model's own output, and nothing produced one), and a queued asynchronous submission
leaves `InProgress` for the modelled failure state once its outcome falls due, with a
reason that says no container ran it — never a `Completed` claiming an output object
nothing wrote. The echo/weighted/fixed "model behaviour" registry, the stateful-session
lifecycle and the custom-attribute echo this issue asked for all depend on a container's
behaviour and are answered by those modelled errors instead.

The crate's `tests/all/` suites cover it end to end.

Accepted follow-ups, each worth its own issue:

- the data plane reports no `InvokedProductionVariant` and no weighted distribution,
  because nothing routes an invocation to a variant; if the control plane ever grows
  variant weights worth honouring, the choice belongs beside `resolve_endpoint`;
- an asynchronous submission's `RequestTTLSeconds` expiry is stored but not yet
  materialised as its own clock-owed outcome (the queue settles on the no-container
  failure first);
- `service-com-amazonaws-sagemakerruntimehttp2-amazonsagemakerruntimehttp2` still carries
  its own endpoint registry and model behaviour, which should be reconciled with this
  control-plane resolution rather than left as a second, different answer.
