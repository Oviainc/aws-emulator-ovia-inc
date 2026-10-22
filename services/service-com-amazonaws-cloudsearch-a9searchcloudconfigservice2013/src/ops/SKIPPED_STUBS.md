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

// SKIPPED: ListDomainNames — unsupported shape construct: operation 'ListDomainNames' has a Unit/absent input, which the generator does not emit yet
//
// RESOLVED by hand: `ListDomainNames` is now implemented in
// `src/ops/list_domain_names.rs`, with its `Unit` input and its response shape
// declared in `src/ops/types/list_domain_names_types.rs`. The generator still
// declines to emit it, so this entry stays as the record of WHY that file is
// hand-authored; the build script discovers it from its `//! @op` header like any
// other operation, so it is routed and served normally.
