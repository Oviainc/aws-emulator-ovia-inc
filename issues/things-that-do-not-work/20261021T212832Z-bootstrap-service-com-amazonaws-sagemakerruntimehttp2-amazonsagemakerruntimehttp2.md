Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the SageMaker Runtime HTTP2 emulator

_Filed: 2026-10-21T21:28:32Z_
_Operation: service-com-amazonaws-sagemakerruntimehttp2-amazonsagemakerruntimehttp2 | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has NO working handler at all: `src/ops/` contains only `mod.rs` plus `SKIPPED_STUBS.md`, which records that the single modeled operation was declined by the stub generator (`// SKIPPED: InvokeEndpointWithBidirectionalStream — unsupported shape construct: ... @streaming event-stream input member, which needs wire framing the generator does not emit`). The operation is therefore modeled-but-unwired and is answered by the framework's unroutable-operation path, so no client can do anything with this service.

The service binds exactly one operation, so the bootstrap happy path is that one bidirectional streaming invocation, hand-authored end to end. The first slice is:

1. **Seed/own an endpoint registry (state, no public op)** — the data-plane model has no control-plane op, so the emulator must keep a registry of endpoint names (seeded at state construction and/or writable by test helper) with their production variants. Invoking an unknown `EndpointName` must NOT succeed.
2. **InvokeEndpointWithBidirectionalStream** — `POST /endpoints/{EndpointName}/invocations-bidirectional-stream`.
   - Input: `EndpointName` (@httpLabel, required, maxLen 63, pattern `^[a-zA-Z0-9](-*[a-zA-Z0-9])*`), `Body` (@httpPayload, required, @streaming union `RequestStreamEvent` with member `PayloadPart` → `RequestPayloadPart{Bytes (@eventPayload blob, sensitive), DataType (@eventHeader, `^(UTF8)$|^(BINARY)$`), CompletionState (@eventHeader, `^(PARTIAL)$|^(COMPLETE)$`), P (@eventHeader)}`), and headers `TargetVariant` (X-Amzn-SageMaker-Target-Variant), `ModelInvocationPath` (X-Amzn-SageMaker-Model-Invocation-Path), `ModelQueryString` (X-Amzn-SageMaker-Model-Query-String).
   - Output: `Body` (@httpPayload, required, @streaming union `ResponseStreamEvent` with members `PayloadPart` → `ResponsePayloadPart{Bytes, DataType, CompletionState, P}`, `ModelStreamError`, `InternalStreamFailure`) and header `InvokedProductionVariant` (X-Amzn-Invoked-Production-Variant).
3. **Request-side event-stream framing** — decode `vnd.amazon.event-stream` frames from the request body into `RequestStreamEvent::PayloadPart` values, preserving each frame's event headers and payload bytes. Multiple frames in one call must all be consumed, in order, until a frame with `CompletionState=COMPLETE` (or end of body).
4. **Response-side event-stream framing** — emit a frame sequence computed from the accumulated request frames and the endpoint's configured model behaviour: one or more `PayloadPart` frames with `CompletionState=PARTIAL` followed by a final `COMPLETE` frame, each carrying `DataType` and `Bytes`. `InvokedProductionVariant` must be the variant actually selected (the `TargetVariant` when supplied and valid, otherwise the registry's default variant) — read from state, not hardcoded.
5. **Modeled error paths** — unknown/invalid `EndpointName`, header value violating the modeled length/pattern constraints, or a request frame whose `DataType`/`CompletionState` violates its pattern → `InputValidationError` (HTTP 400) with `Message`/`ErrorCode`; endpoint configured to fail → `ModelError` (HTTP 424) with `Message`, `OriginalStatusCode`, `OriginalMessage`, `LogStreamArn`, `ErrorCode`; a mid-stream model failure must be delivered as a `ModelStreamError`/`InternalStreamFailure` member INSIDE the response event stream (after a 200 and after earlier frames were already flushed), not as a top-level HTTP error.

Every one of these must compute its response from persisted state — no canned bodies, no blind echo of the request payload, no hardcoded `InvokedProductionVariant`. The invocation transcript must be observable after the call (so a test can assert the frames the emulator received and the frames it produced), and the endpoint registry must be shared through `crate::state::service_state(&*self.engine)` so successive calls see the same store (see README.md "State": never construct a fresh engine in a handler).

## State Model

- `ComAmazonawsSagemakerruntimehttp2Amazonsagemakerruntimehttp2State`
  - `endpoints: HashMap<String /*EndpointName*/, Endpoint>`
  - `invocations: Vec<Invocation>` (ordered transcript, newest last)
- `Endpoint { name: String, production_variants: Vec<String>, default_variant: String, behaviour: ModelBehaviour, created_at: SystemTime }`
- `ModelBehaviour` (how the fake model answers; selected per endpoint): `Echo { chunk_size: usize, data_type: String }` | `Scripted { frames: Vec<ResponseFrame> }` | `FailWithModelError { original_status_code: i32, original_message: String, log_stream_arn: String, error_code: String }` | `FailMidStream { frames_before_failure: usize, stream_error: StreamFailureKind }`
- `RequestFrame { bytes: Vec<u8>, data_type: Option<String>, completion_state: Option<String>, p: Option<String> }`
- `ResponseFrame { bytes: Vec<u8>, data_type: Option<String>, completion_state: Option<String>, p: Option<String> }`
- `Invocation { endpoint_name: String, target_variant: Option<String>, invoked_production_variant: String, model_invocation_path: Option<String>, model_query_string: Option<String>, request_frames: Vec<RequestFrame>, response_frames: Vec<ResponseFrame>, terminal: InvocationOutcome, started_at: SystemTime }`
- `InvocationOutcome { Completed, ModelStreamError { message, error_code }, InternalStreamFailure { message } }`

Anything beyond this slice (richer model behaviours, auth/sigv4 nuances, ServiceUnavailableError throttling simulation) stays out of scope and is filed as individual gap issues later.

## Evidence

OPERATIONS.md (crate root) lists exactly 1 operation, `InvokeEndpointWithBidirectionalStream`, with route `POST /endpoints/{EndpointName}/invocations-bidirectional-stream`, required `@httpLabel` `EndpointName`, required `@httpPayload` streaming `Body`, the three `X-Amzn-SageMaker-*` request headers, output `Body` + `X-Amzn-Invoked-Production-Variant`, and errors InputValidationError(400)/InternalServerError(500)/InternalStreamFailure(500)/ModelError(424)/ModelStreamError(400)/ServiceUnavailableError(503). `smithy_query closure` confirms the member constraints (EndpointName maxLen 63 + pattern, DataType `^(UTF8)$|^(BINARY)$`, CompletionState `^(PARTIAL)$|^(COMPLETE)$`, Bytes `@eventPayload` sensitive blob) and that `ResponseStreamEvent` carries `ModelStreamError`/`InternalStreamFailure` as in-stream members. `src/ops/SKIPPED_STUBS.md` proves there is no handler file for the operation (declined: streaming event-stream input member), i.e. the service is 100% non-functional. Protocol is `restJson1` with event-stream framing; sigv4 signing name `sagemaker`, endpoint prefix `runtime.sagemaker` (OPERATIONS.md "Wire protocol"). No tests exist beyond the generated `tests/state_roundtrip.rs` example.

## Steps to Reproduce

1. Start the emulator and register/seed an endpoint named `my-endpoint` with production variants [`AllTraffic`] in the service state.
2. Open `POST /endpoints/my-endpoint/invocations-bidirectional-stream` with `Content-Type: application/vnd.amazon.eventstream` and send two request frames: `PayloadPart{DataType: UTF8, CompletionState: PARTIAL, Bytes: "hello "}` then `PayloadPart{DataType: UTF8, CompletionState: COMPLETE, Bytes: "world"}`.
3. Expect HTTP 200 with header `X-Amzn-Invoked-Production-Variant: AllTraffic` (taken from the endpoint registry, not hardcoded) and a response event stream of one or more `PayloadPart` frames whose concatenated `Bytes` are derived from the two request frames, with the last frame carrying `CompletionState: COMPLETE`. Currently the request is not routed at all (framework unroutable-operation path / 501).
4. Read back the stored invocation transcript and confirm both request frames and all emitted response frames were persisted in order with their event headers.
5. Repeat against `POST /endpoints/does-not-exist/invocations-bidirectional-stream` and expect `InputValidationError` with HTTP 400 and a populated `Message`/`ErrorCode`, not a success.
6. Send `X-Amzn-SageMaker-Target-Variant: not a variant!!` (pattern violation) and expect HTTP 400 `InputValidationError`.
7. Configure the endpoint's behaviour to fail mid-stream after 1 frame, invoke again, and expect HTTP 200 whose event stream contains a leading `PayloadPart` frame followed by a `ModelStreamError` (or `InternalStreamFailure`) member frame — not a top-level HTTP error.
8. Configure the endpoint's behaviour to `FailWithModelError`, invoke, and expect HTTP 424 `ModelError` with `OriginalStatusCode`, `OriginalMessage` and `LogStreamArn` populated from the configured state.
