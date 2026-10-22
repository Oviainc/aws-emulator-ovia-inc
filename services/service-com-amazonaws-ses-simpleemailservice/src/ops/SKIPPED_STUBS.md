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

// SKIPPED: GetAccountSendingEnabled — unsupported shape construct: operation 'GetAccountSendingEnabled' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: GetSendStatistics — unsupported shape construct: operation 'GetSendStatistics' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: ListVerifiedEmailAddresses — unsupported shape construct: operation 'ListVerifiedEmailAddresses' has a Unit/absent input, which the generator does not emit yet

// HAND-AUTHORED since: GetSendQuota — the generator declined it for the same
// Unit-input reason, and it is now written out in `ops/get_send_quota.rs` (with
// its two shapes in `ops/types/get_send_quota_types.rs`), so it is typed, in the
// service's `operations(...)` allow-list, and served like every other operation.
