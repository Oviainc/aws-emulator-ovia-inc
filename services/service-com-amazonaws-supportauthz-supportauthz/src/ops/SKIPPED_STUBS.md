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

// SKIPPED: CreateSupportPermit — shape missing from model: smithy.api#Unit
// SKIPPED: DeleteSupportPermit — shape missing from model: smithy.api#Unit
// SKIPPED: GetSupportPermit — shape missing from model: smithy.api#Unit
// SKIPPED: ListSupportPermitRequests — shape missing from model: smithy.api#Unit

## Hand-authored since

`CreateSupportPermit`, `DeleteSupportPermit`, `GetSupportPermit` and
`ListSupportPermits` are now implemented in `ops/<op>.rs` and bound to the
service: the decline reason was that `ActionSet` and `ResourceSet` each carry a
union member targeting the Smithy PRELUDE shape `smithy.api#Unit`, which the
JSON AST does not serialize, so the generator could not type it. The model now
carries the prelude's own definition of that shape and the four handlers are
written by hand.

`ListSupportPermitRequests` is still unwired and answers the framework's
unroutable-operation path; it will be filed as its own gap issue.

// SKIPPED: ListSupportPermits — shape missing from model: smithy.api#Unit
