Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 81fe993c0fc493a00f42b69c764d54e4324ad80b

# The cursor codec's scope/key boundary is unescaped, so a page token issued for ANOTHER account's role listing is honoured instead of refused

_Filed: 2026-11-06T14:57:26Z_
_Operation: ListAccountRoles | Class: pagination_

> _Note: this issue was identified against the code base at commit `81fe993c0fc493a00f42b69c764d54e4324ad80b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state`'s cursor codec encodes a cursor as the single string `"{CURSOR_VERSION}:{scope}:{last_key}"` and decodes it by `strip_prefix("{CURSOR_VERSION}:{scope}:")`. Neither the scope nor the key is escaped, and for `ListAccountRoles` the scope embeds a CALLER-SUPPLIED value: `cursor_scope(&format!("roles:{}", account.account_id), &input.access_token)`. `AccountIdType` is an unconstrained string (no `@pattern`, no `@length`), so an account id may itself contain the `:` separator — and when it does, the boundary between "scope" and "resume key" inside a decoded cursor is ambiguous.

Concretely, for accounts `B` and `A = B + ":" + H + ":" + R` (where `H` is the 32-hex session digest `cursor_scope` appends and `R` is any string), the cursor minted for A's role listing decodes under B's scope: stripping `"v1:roles:{B}:{H}:"` from `"v1:roles:{B}:{H}:{R}:{H}:{key}"` succeeds and yields the resume key `"{R}:{H}:{key}"`. The guard that is documented as what holds the codec to exact inverses — `if encode_cursor(scope, last_key) != token { return Err(InvalidCursor) }` — cannot catch this, because re-minting the MIS-SPLIT key reproduces the very same bytes. So the foreign cursor is accepted and `page_of` resumes account B's role listing after a key that was never one of B's role names: the request answers HTTP 200 with a silently truncated (typically empty) `roleList` and no `nextToken`, instead of the modelled 400 `InvalidRequestException` the handler's own comment promises.

Severity is LOW and the scope is stated honestly: both listings belong to the SAME sign-in session (the digest `H` is derived from the one presented access token), so nothing is reported that this session's own listing could not already report — `state.rs:415-423` already documents that a caller able to reproduce the encoding can resume at a key of its choosing. What is defective is the CROSS-LISTING scoping that the crate claims three separate times to enforce, and the resulting silent loss of a page the caller is entitled to see. Reaching it requires an account id that embeds the token digest, i.e. a contrived seeding — the same "unreachable from an ordinary client, reachable through the public `state` API" scope the already-fixed `page_of` clamp issue was filed under.

## Evidence

The rule the handler states for itself — services/service-com-amazonaws-sso-swbportalservice/src/ops/list_account_roles.rs:199-207:
```rust
        // The cursor is scoped to this account's role listing AND to this session, so a
        // token issued for another account's roles, or to another sign-in, cannot be
        // continued here.
        let page = crate::state::page_of(
            assigned,
            &crate::state::cursor_scope(
                &format!("roles:{}", account.account_id),
                &input.access_token,
            ),
```
The same rule, stated twice more in the module that owns the codec — src/state.rs:404-413:
```rust
/// The scope a cursor is bound to: the listing it walks, and the session that asked
/// for it. A cursor presented to another listing or by another session is refused.
...
pub fn cursor_scope(listing: &str, access_token: &str) -> String {
    let digest = <sha2::Sha256 as sha2::Digest>::digest(access_token.as_bytes());
    format!("{listing}:{}", hex::encode(&digest[..16]))
}
```
and src/state.rs:428-437:
```rust
/// The key a cursor resumes after, when it is one [`encode_cursor`] produced for the
/// same `scope`. A cursor from another listing (a different account's roles, or another
/// session's accounts) is refused rather than applied to this one.
///
/// Admitted ONLY when the token is byte for byte the one [`encode_cursor`] mints for the
/// key it carries: the acceptance set is the mint's output set and nothing wider.
```
The code that cannot keep that promise — the unescaped concatenation (src/state.rs:424-426):
```rust
pub fn encode_cursor(scope: &str, last_key: &str) -> String {
    hex::encode(format!("{CURSOR_VERSION}:{scope}:{last_key}"))
}
```
and the prefix-strip plus re-mint check (src/state.rs:438-448):
```rust
pub fn decode_cursor(scope: &str, token: &str) -> Result<String, InvalidCursor> {
    let bytes = hex::decode(token).map_err(|_| InvalidCursor)?;
    let decoded = String::from_utf8(bytes).map_err(|_| InvalidCursor)?;
    let last_key = decoded
        .strip_prefix(&format!("{CURSOR_VERSION}:{scope}:"))
        .ok_or(InvalidCursor)?;
    if encode_cursor(scope, last_key) != token {
        return Err(InvalidCursor);
    }
    Ok(last_key.to_string())
}
```
Re-minting the mis-split key yields the identical string, so the `encode_cursor(scope, last_key) != token` guard passes: for `A = B + ":" + H + ":" + R`, `encode_cursor("roles:{B}:{H}", "{R}:{H}:{key}")` == `encode_cursor("roles:{A}:{H}", "{key}")`.

The model permits the account id that triggers it — `smithy_query(query="shape", shape="AccountIdType")`:
```
"AccountIdType": {"shape_id": "com.amazonaws.sso#AccountIdType", "type": "string"}
```
(no pattern, no length), and the crate's own seeding API takes it verbatim — src/state.rs:203-217 (`put_account(account_id: impl Into<String>, ...)` → `self.accounts.insert(account_id, account)`), with `Account.account_id` then flowing straight into the scope at src/ops/list_account_roles.rs:205.

Test coverage: the intended rule IS pinned, but only for account ids that contain no separator — tests/all/portal_session.rs:361-364 (`RULE: ... a cursor from another account's listing is not a cursor for this one (400)`) and tests/all/page_token_spelling.rs:47-77, whose `ASSIGNED` account ids are all plain digit strings (`"501111111111"`, …). No test puts a `:` in an account id, so the ambiguous split is unpinned on both paginated handlers.

## Steps to Reproduce

1. Stand the service up as tests/all/portal_session.rs:85-114 does (an `InMemoryEngine` plus `EmulatedService` on a `MultiServiceRouter`).
2. Pick a token `T = "tok-x"` and compute `H = hex::encode(&sha2::Sha256::digest(b"tok-x")[..16])` — the same 32 hex characters `state::cursor_scope` appends (src/state.rs:410-413).
3. Seed two accounts through the crate's public seeding API: `put_account("111111111111", "B", "b@example.com", ["AdministratorAccess", "ReadOnly"])` and `put_account(format!("111111111111:{H}:Z"), "A", "a@example.com", ["RoleOne", "RoleTwo"])`, then `put_session("tok-x", "x@example.com", ["111111111111", format!("111111111111:{H}:Z")], ttl)`.
4. GET `/assignment/roles?account_id=111111111111:{H}:Z&max_result=1` with `x-amz-sso_bearer_token: tok-x` and keep the `nextToken` it returns (it is `hex("v1:roles:111111111111:{H}:Z:{H}:RoleOne")`).
5. Present that token to the OTHER account's listing: GET `/assignment/roles?account_id=111111111111&max_result=10&next_token=<that token>` with the same bearer token.
6. Observed: HTTP 200 with `roleList: []` and no `nextToken` — `decode_cursor` stripped `"v1:roles:111111111111:{H}:"`, handed `page_of` the resume key `"Z:{H}:RoleOne"`, and `page_of` (src/state.rs:490-500) skipped every role of account 111111111111 because `"AdministratorAccess" <= "Z:…"` and `"ReadOnly" <= "Z:…"`. Account 111111111111's roles are reported as absent.
7. Expected per src/ops/list_account_roles.rs:199-201 ("a token issued for another account's roles … cannot be continued here"), src/state.rs:404-405 and src/state.rs:429-430: the foreign cursor is refused with 400 `InvalidRequestException` and the message tests/all/page_token_spelling.rs:36 pins, exactly as the numeric-account-id case in tests/all/portal_session.rs:420-443 already is.
