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

// SKIPPED: InvokeEndpointWithResponseStream — unsupported shape construct: operation 'InvokeEndpointWithResponseStream' has a @streaming event-stream output member, which needs wire framing the generator does not emit
//
// HAND-AUTHORED SINCE, so this crate has no unwired operation left: the generator still
// declines the operation (the line above is the generator's own record of why), but
// `ops/invoke_endpoint_with_response_stream.rs` now implements it and `service.rs` routes
// the model's `POST /endpoints/{EndpointName}/invocations-response-stream` to it through
// `Service::override_operation`, so a request on that route is served by this crate rather
// than by the framework's unroutable-operation path.
