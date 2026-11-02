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

// SKIPPED: InvokeHarness — unsupported shape construct: operation 'InvokeHarness' has a @streaming event-stream output member, which needs wire framing the generator does not emit
// SKIPPED: InvokeAgentRuntimeCommand — unsupported shape construct: operation 'InvokeAgentRuntimeCommand' has a @streaming event-stream output member, which needs wire framing the generator does not emit

The operation below was skipped for the same reason — a `@streaming` event-stream
OUTPUT member — and is now HAND-AUTHORED, so it is no longer unwired: it has its own
file in `src/ops/`, the shapes its request and its one event need are in
`src/ops/types/invoke_code_interpreter_types.rs`, the `vnd.amazon.eventstream` framing
is the shared `service_common::event_stream`, and `src/service.rs` routes a request to
it through `Service::override_operation` — which the router consults BEFORE the
generated routing table, whose shim for a `@streaming`-output operation declines the
request.

`InvokeHarness` and `InvokeAgentRuntimeCommand` above are NOT implemented and still
answer the framework's "modeled but not served" response; each has its own gap issue.

// HAND-AUTHORED: InvokeCodeInterpreter — src/ops/invoke_code_interpreter.rs
