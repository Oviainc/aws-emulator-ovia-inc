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

No operation of this service is left modeled-but-unwired.

The four harness operations USED to be listed here, each declined as
"shape missing from model: smithy.api#Unit": the `Harness` closure reaches
`HarnessGatewayOutboundAuth`, a union two of whose members target the
`smithy.api#Unit` prelude shape, which the model JSON does not carry. All four
have since been hand-authored — `ops/create_harness.rs`, `ops/get_harness.rs`,
`ops/delete_harness.rs` and `ops/update_harness.rs` (a 501 stub), over the
shapes in `ops/types/harness_types.rs`, which carries that one union's
hand-written codec — so every route this service's model binds now resolves to a
handler instead of being refused by the router as an unknown service.
