Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 49d3116d0296255c615a3da18aeed15b4a7eb9dc

# SampleWithResponseStream delivers hand-rolled server-sent events instead of the PayloadPart events its model documents

_Filed: 2026-11-04T09:23:34Z_
_Operation: SampleWithResponseStream | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `49d3116d0296255c615a3da18aeed15b4a7eb9dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents the streaming response body of `SampleWithResponseStream` as a series of `PayloadPart` events. The handler instead renders the completion as OpenAI-style server-sent events — one `data: <json>` line plus a `data: [DONE]` sentinel — and labels the response `Content-Type: text/event-stream`, a framing and media type the crate authors for itself (its own comment states the rule it chose: "OpenAI-compatible streaming is server-sent events"). A client reading the modelled response therefore receives a framing the model does not describe, under a media type that is not the one an event-delivered body carries, and the whole completion arrives as a single chunk rather than as a series of events. The crate also never reaches the shared event-stream framing support that already exists in this tree (`services/common/src/event_stream.rs`, `EVENT_STREAM_CONTENT_TYPE = "application/vnd.amazon.eventstream"`), which is what sibling streaming data planes in this repo serialize an event body with.

## Evidence

Model (smithy_query query="doc" shape="SampleWithResponseStreamResponse" member="Body"): "The streaming response body, delivered as a series of PayloadPart events." and smithy_query query="doc" shape="SampleWithResponseStream": "Sends a streaming inference request to the model during a job execution. Returns the response as a stream of payload chunks."

Actual, services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/state.rs:80-83:
```
/// The wire value of the `Content-Type` a streamed inference response is
/// rendered in: OpenAI-compatible streaming is server-sent events, one
/// `chat.completion.chunk` per `data:` line and a final `data: [DONE]`.
pub const EVENT_STREAM_CONTENT_TYPE: &str = "text/event-stream";
```
and src/state.rs:980-982:
```
    let envelope = completion_envelope(job_arn, trajectory_id, index, prompt, created, true);
    let chunk = serde_json::to_string(&envelope).unwrap_or_default();
    format!("data: {chunk}\n\ndata: [DONE]\n\n").into_bytes()
```
reached from the handler at src/ops/sample_with_response_stream.rs:196-203:
```
                    crate::state::sample_stream_response_body(
                        &job_arn,
                        &trajectory_id,
                        index,
                        &prompt,
                        now,
                    ),
                    crate::state::EVENT_STREAM_CONTENT_TYPE.to_string(),
```

Shared framing the crate does not use, services/common/src/event_stream.rs:56: `pub const EVENT_STREAM_CONTENT_TYPE: &str = "application/vnd.amazon.eventstream";` ("`application/vnd.amazon.eventstream` wire support for an operation whose OUTPUT …", services/common/src/event_stream.rs:3).

Test coverage, disclosed: tests/all/trajectory_lifecycle.rs:297-300 pins the current framing —
```
                    text.starts_with("data: ") && text.ends_with("data: [DONE]\n\n"),
                    "case {case:?} turn {index}: expected server-sent-event framing, got {text:?}"
```
— and tests/all/job_resolution.rs:240 asserts the served `Content-Type` equals the crate's OWN `state::EVENT_STREAM_CONTENT_TYPE` constant rather than any externally stated value, so neither assertion records a reason for the divergence from the member's documented delivery.

## Steps to Reproduce

1. Register an AgentRFT job in the shared job directory (as tests/all/harness.rs `create_job` does) so the `JobArn` resolves.
2. POST /sample-with-response-stream with headers `X-Amzn-SageMaker-Job-Arn: arn:aws:sagemaker:us-east-1:000000000000:job/AgentRFT/rollout-1`, `X-Amzn-SageMaker-Trajectory-Id: traj-1` and an OpenAI-compatible JSON payload as the body.
3. Observe the 200 response: `Content-Type: text/event-stream` and a body of `data: {"object":"chat.completion.chunk",...}\n\ndata: [DONE]\n\n`.
4. Compare with the modelled member documentation for `SampleWithResponseStreamResponse$Body` — "delivered as a series of PayloadPart events" — which describes event delivery, not `data:`-prefixed server-sent-event text.
