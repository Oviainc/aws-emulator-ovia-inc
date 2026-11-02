Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 3ca7a48f5c7417112003e42cf25e2b2527498f37

# The captured turn's request Content-Type is read twice, once per sampling handler, outside the crate's one sampling path

_Filed: 2026-10-30T02:05:00Z_
_Operation: Sample | Class: maintainability_

> _Note: this issue was identified against the code base at commit `3ca7a48f5c7417112003e42cf25e2b2527498f37`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Sample` and `SampleWithResponseStream` each contain their own identical copy of the computation that produces `TurnRequest::request_content_type` — the `Content-Type` request header lookup, the `to_str().ok()` filter and the `map(str::to_string)` conversion. Everything else the two sampling operations share was deliberately single-homed in `crate::state::serve_sampling_turn` / `TurnRequest`, and both handlers plus `state.rs` state in prose that the ONLY things left to each operation are the response body it renders, the `Content-Type` that body carries, the `streamed` flag and the modelled error enum it reports a refusal through. Reading the REQUEST's `Content-Type` is not on that list, yet it lives in both handlers, so the two copies can drift (one handler could start normalising, lowercasing or defaulting the value) and the captured turns of ONE trajectory — which is explicitly allowed to mix turns served by either operation — would then record the same request header two different ways. The existing suites assert the captured value for turns served by both operations, so folding the read into the one sampling path (e.g. computing it once where `TurnRequest` is built, from the request headers the shared path is handed) leaves every response and every existing assertion unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/ops/sample.rs:172-177:
```
        let request_content_type = ctx
            .parts()
            .headers
            .get(::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
```

Copy 2 — src/ops/sample_with_response_stream.rs:173-178 (byte-for-byte the same):
```
        let request_content_type = ctx
            .parts()
            .headers
            .get(::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
```

The crate's own statement of what may differ per operation (so this read should not be handler-local):
- src/state.rs:704-710 (`serve_sampling_turn`): "The two operations reach the SAME state through this, so one trajectory can mix turns served by either of them and neither can drift from the other on which calls are refused, in which order, or with which sentence. What is left to each of them is what genuinely differs: the response body it renders (`respond`), the `Content-Type` that body carries, the `streamed` flag the captured turn records, and the modelled error enum it reports a [`SamplingRefusal`] through."
- src/ops/sample.rs:183-187: "The pre-flight and the capture are the crate's ONE sampling path (`crate::state::serve_sampling_turn`), shared with `SampleWithResponseStream`; what is this operation's own is the non-streaming body it renders and the modelled errors it reports a refusal through."
- src/ops/sample_with_response_stream.rs:184-187: "The SAME state path as `Sample` — the crate's ONE sampling path (`crate::state::serve_sampling_turn`) — so one trajectory can mix turns served by either operation and the two cannot drift; only the framing of the response body differs."

The value both copies feed, and that the store retains per turn:
- src/state.rs:164-166 (`Turn`): "/// The `Content-Type` the caller labelled the request body with, when it\n    /// sent one.\n    pub request_content_type: Option<String>,"
- src/ops/sample.rs:191-197 and src/ops/sample_with_response_stream.rs:191-198 both pass `request_content_type` into `crate::state::TurnRequest { .. }`.

Existing tests that pin the behaviour the consolidation must not change (so the fix is response- and test-neutral):
- tests/all/trajectory_lifecycle.rs:273-279 asserts, for every captured turn of rows that mix `Sample` and `SampleWithResponseStream`: `assert_eq!(turn.request_content_type.as_deref(), Some("application/json"), "case {case:?} turn {index}: expected the request Content-Type to be captured, got {:?}", turn.request_content_type);`
- tests/all/sampling_path_is_shared.rs:3-10 states the rule the duplicate sits against: "`Sample` and `SampleWithResponseStream` are ONE path through the store. RULE: both sampling operations run the same pre-flight in the same order and capture the turn through the same store call … what differs between them is only the framing of a served body — the `Content-Type` it carries and whether the captured turn is recorded as streamed." (that suite compares the two wire answers but never reads `request_content_type`, so a drift in these two copies is unpinned).

## Steps to Reproduce

1. Open services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/ops/sample.rs and read lines 172-177.
2. Open src/ops/sample_with_response_stream.rs and read lines 173-178: the same six-line computation of `request_content_type` appears again, verbatim.
3. Read src/state.rs:704-710 and the two handler comments (sample.rs:183-187, sample_with_response_stream.rs:184-187): the crate states the per-operation differences as the rendered body, its `Content-Type`, the `streamed` flag and the error enum — the request header read is not among them, and every other shared step lives in `serve_sampling_turn`/`TurnRequest`.
4. Confirm the behaviour is pinned for both operations by tests/all/trajectory_lifecycle.rs:273-279, so hoisting the read into the one sampling path changes no response and no assertion.
