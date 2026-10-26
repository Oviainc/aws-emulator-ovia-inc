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

// HAND-AUTHORED since: GetSendQuota — the generator declined it for the same
// Unit-input reason, and it is now written out in `ops/get_send_quota.rs` (with
// its two shapes in `ops/types/get_send_quota_types.rs`), so it is typed, in the
// service's `operations(...)` allow-list, and served like every other operation.

// HAND-AUTHORED since: GetAccountSendingEnabled — written out in
// `ops/get_account_sending_enabled.rs`, shapes in
// `ops/types/get_account_sending_enabled_types.rs`.

// HAND-AUTHORED since: GetSendStatistics — written out in
// `ops/get_send_statistics.rs`, shapes in
// `ops/types/get_send_statistics_types.rs`.

// HAND-AUTHORED since: ListVerifiedEmailAddresses — written out in
// `ops/list_verified_email_addresses.rs`, shapes in
// `ops/types/list_verified_email_addresses_types.rs`.
