# Skipped operation stubs

The scaffold op-stub generator declined to emit a stub for the operations
below. Every remaining decline class is one the generator cannot TYPE: a
Unit/absent input, a `@streaming` member (event-stream union or streaming
blob), a member whose target is a shape kind that has no Rust type, or a
malformed model. A stub that does not compile would break the tenant build,
so each is left modeled-but-unwired — hand-author it in `ops/<op>.rs`, using
a rendered sibling as the pattern.

Note the difference from a SCAFFOLDED operation. Both answer HTTP 501, but a
scaffolded op has a compile-checked typed skeleton and is in the service's
`operations(...)` allow-list; an op listed here has neither, and is answered
by the framework's unroutable-operation path instead.

An output that is a SCALAR, list or map is declined: only a structure or a
union can be declared as `{Op}Output`. A `@required` union member and a
`@required`-member reference cycle are NOT decline reasons — a scaffolded
handler returns an error, so it never has to construct a value.

// SKIPPED: InvokeModelWithBidirectionalStream — unsupported shape construct: operation 'InvokeModelWithBidirectionalStream' has a @streaming event-stream input member, which needs wire framing the generator does not emit

The two operations below were skipped for the same reason — a `@streaming`
event-stream OUTPUT member — and are now HAND-AUTHORED, so they are no longer
unwired: each has its own file in `src/ops/`, the shapes their request and their
events need are in `src/ops/types/streaming_types.rs`, the
`vnd.amazon.eventstream` framing is the shared `service_common::event_stream`, and
`src/service.rs` routes a request to one through `Service::override_operation` —
which the router consults BEFORE the generated routing table, whose shim for a
`@streaming`-output operation declines the request.

Each serves the SAME inference its non-streaming sibling serves — the same model
resolution, the same validation, the same tokenizer and the same response envelope
out of `src/state.rs` — framed as the operation's own events. No language model
runs in this emulator, so each completion is produced in one step and arrives as
one delta (`ConverseStream`) or one `chunk` (`InvokeModelWithResponseStream`);
nothing invents incremental generation.

`InvokeModelWithBidirectionalStream` above is NOT implemented and still answers the
framework's "modeled but not served" response; it has its own gap issue.

// HAND-AUTHORED: ConverseStream — src/ops/converse_stream.rs
// HAND-AUTHORED: InvokeModelWithResponseStream — src/ops/invoke_model_with_response_stream.rs
