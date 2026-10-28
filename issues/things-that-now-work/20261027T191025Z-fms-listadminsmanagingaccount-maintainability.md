Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 74cfda6b1b1701e11661fc7b5d0b64a8c57e063b

# decode_account_page_token re-implements the hex/UTF-8 token decode that decode_key_token already owns

_Filed: 2026-10-27T19:10:25Z_
_Operation: ListAdminsManagingAccount | Class: maintainability_

> _Note: this issue was identified against the code base at commit `74cfda6b1b1701e11661fc7b5d0b64a8c57e063b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` has one helper that turns a `PaginationToken` back into a store key — `decode_key_token` — and three of the four specialised decoders delegate to it, adding only their own shape check. `decode_account_page_token` does not: it copies the `::hex::decode` + `String::from_utf8` step and then applies `is_account_id`. The decode rule is therefore written twice in this crate's own code and the two copies can drift (a change to how a token is encoded, e.g. a different encoding or a prefix, has to be mirrored by hand in the second copy or the account-keyed pages silently start refusing tokens this service minted).

The fix is a one-line delegation (`let decoded = decode_key_token(token)?;`) and changes no response: `decode_key_token`'s extra guards (non-empty, no control characters) are implied by `is_account_id`, which already requires length 1..=1024 and all-ASCII-digits, so every token accepted or refused today is accepted or refused identically afterwards.

## Evidence

The helper that owns the rule — `src/state.rs:2890-2894`:
```rust
pub fn decode_key_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let is_key = !decoded.is_empty() && !decoded.chars().any(char::is_control);
    is_key.then_some(decoded)
}
```
The second copy of the same two lines — `src/state.rs:2953-2956`:
```rust
pub fn decode_account_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    is_account_id(&decoded).then_some(decoded)
}
```
The three siblings that delegate instead — `src/state.rs:2906` (`decode_page_token`), `src/state.rs:2925` (`decode_base62_page_token`), `src/state.rs:2942` (`decode_uri_page_token`), each opening with:
```rust
    let decoded = decode_key_token(token)?;
```
And `is_account_id` makes the dropped guards redundant — `src/state.rs:2151-2153`:
```rust
pub fn is_account_id(value: &str) -> bool {
    (1..=1024).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_digit())
}
```

Existing tests that pin the behaviour this refactor must not change: `tests/all/admin_accounts_for_organization.rs:419-448` (a self-minted account token resumes; "hex of a non-account string", a policy-keyed token, a Base62 token and "hex of an empty key" are all refused), `tests/all/compliance_status.rs:969-970`, `tests/all/tagging_and_managing_admins.rs:635-660`, plus `tests/all/member_accounts.rs`. Callers: `src/ops/list_admins_managing_account.rs:164`, `src/ops/list_member_accounts.rs:178`, `src/ops/list_admin_accounts_for_organization.rs:187`, `src/ops/list_compliance_status.rs:201`.

## Steps to Reproduce

1. Open services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs and read `decode_key_token` (lines 2890-2894).
2. Read `decode_page_token` (2905), `decode_base62_page_token` (2924) and `decode_uri_page_token` (2941): each begins `let decoded = decode_key_token(token)?;`.
3. Read `decode_account_page_token` (2953-2956): it repeats `String::from_utf8(::hex::decode(token).ok()?).ok()?` instead of calling the helper — the same decode rule expressed twice in one module.
