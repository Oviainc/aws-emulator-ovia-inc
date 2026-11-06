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

// SKIPPED: GetAccountLevelServiceConfiguration — unsupported shape construct: operation 'GetAccountLevelServiceConfiguration' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: GetServiceIndex — unsupported shape construct: operation 'GetServiceIndex' has a Unit/absent input, which the generator does not emit yet

`GetIndex`, `GetDefaultView` and `DisassociateDefaultView` were declined for the same
reason and have since been hand-authored in `src/ops/get_index.rs`,
`src/ops/get_default_view.rs` and `src/ops/disassociate_default_view.rs`, which puts them
in the service's `operations(...)` allow-list (`build.rs` derives that list from each ops
file's `//! @op <Name>` header), so they are no longer answered by the
unroutable-operation path.
