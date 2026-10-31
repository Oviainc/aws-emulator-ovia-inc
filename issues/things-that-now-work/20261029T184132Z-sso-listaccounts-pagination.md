Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 3b929b447a52a39dfc36e4a4cbd93099ed5ef8cf

# decode_cursor honours hex spellings encode_cursor never mints, so a nextToken this service never issued is served as a page cursor

_Filed: 2026-10-29T18:41:32Z_
_Operation: ListAccounts | Class: pagination_

> _Note: this issue was identified against the code base at commit `3b929b447a52a39dfc36e4a4cbd93099ed5ef8cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The portal's page cursor is minted by `crate::state::encode_cursor`, which hex-encodes `"{CURSOR_VERSION}:{scope}:{last_key}"` with `hex::encode` — a function that emits only lowercase hex digits (`0-9a-f`). The matching `decode_cursor` admits a token with `hex::decode`, which accepts uppercase and mixed-case hex as well (`hex::decode` doc: "Both, upper and lower case characters are valid in the input string and can even be mixed"). So an upper- or mixed-case spelling of a cursor is a byte string NO call to `encode_cursor` can ever have produced, yet `decode_cursor` decodes it, strips the `v1:<scope>:` prefix and resumes the listing from the key it carries — the service serves a page for a token it never issued.

That contradicts the rule this module states for itself in three places: `decode_cursor`'s own doc admits a token only "when it is one [`encode_cursor`] produced for the same `scope`"; the `CURSOR_VERSION` doc says a token minted by another encoding "is refused rather than silently misread"; and `InvalidCursor` is documented as "A presented `nextToken` that cannot be a cursor of the listing it was presented to". Both listing handlers then advertise exactly that refusal in their 400 message ("nextToken is not a page token this operation issued for this listing"), which they do not give for a case-shifted token.

Both paginated operations are affected (`ListAccounts` and `ListAccountRoles` both call `page_of` -> `decode_cursor`); filed against `ListAccounts`, the first handler that calls the helper. The defect is that the admission test for a cursor accepts spellings the mint cannot produce, so the "only a token I issued resumes this listing" rule is not actually enforced.

## Evidence

The mint — lowercase-only hex (`services/service-com-amazonaws-sso-swbportalservice/src/state.rs:341-343`):
```rust
pub fn encode_cursor(scope: &str, last_key: &str) -> String {
    hex::encode(format!("{CURSOR_VERSION}:{scope}:{last_key}"))
}
```

The stated admission rule (`src/state.rs:346-348`):
```
/// The key a cursor resumes after, when it is one [`encode_cursor`] produced for the
/// same `scope`. A cursor from another listing (a different account's roles, or another
/// session's accounts) is refused rather than applied to this one.
```
and (`src/state.rs:318-319`):
```
/// The version stamped into a cursor, so a token minted by an older encoding is
/// refused rather than silently misread.
```
and (`src/state.rs:312-315`):
```
/// A presented `nextToken` that cannot be a cursor of the listing it was presented to:
/// not a cursor at all, or one scoped to a different listing or a different session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidCursor;
```

The code that breaks it (`src/state.rs:348-355`):
```rust
pub fn decode_cursor(scope: &str, token: &str) -> Result<String, InvalidCursor> {
    let bytes = hex::decode(token).map_err(|_| InvalidCursor)?;
    let decoded = String::from_utf8(bytes).map_err(|_| InvalidCursor)?;
    decoded
        .strip_prefix(&format!("{CURSOR_VERSION}:{scope}:"))
```
`hex::decode` (hex crate, already in Cargo.lock:3177) is case-insensitive, so `hex::decode("7631...".to_uppercase())` succeeds and yields the same bytes — a token no `encode_cursor` call ever returned.

The refusal the handlers claim to give (`src/ops/list_accounts.rs:198-205`, identically `src/ops/list_account_roles.rs:209-216`):
```rust
.map_err(|_| {
    ListAccountsErrorModeled::InvalidRequestException(InvalidRequestException {
        message: Some(
            "nextToken is not a page token this operation issued for this listing"
                .to_string(),
        ),
    })
})?;
```

Test coverage: `tests/all/portal_session.rs:915-921` sweeps the refused-cursor rows — `"not-hex"`, `hex::encode("v1:accounts")`, `hex::encode("v0:accounts:111111111111")`, another account's cursor and another session's cursor — and asserts 400 `InvalidRequestException` for each. No row varies the hex CASE of a cursor, which is why the gap is unpinned.

## Steps to Reproduce

1. Seed the portal store as `tests/all/portal_session.rs` does: `put_account` for at least two accounts and `put_session("tok-live", …, [both account ids], ttl)`.
2. GET `/assignment/accounts?max_result=1` with header `x-amz-sso_bearer_token: tok-live` and copy the `nextToken` from the 200 body (lowercase hex, as `hex::encode` mints it).
3. Upper-case every character of that token and GET `/assignment/accounts?next_token=<UPPERCASED TOKEN>` with the same bearer header.
4. Observed: 200 with the second page — the listing resumes from a token this service never issued. Expected per `decode_cursor`'s own doc ("when it is one encode_cursor produced"): 400 `InvalidRequestException` "nextToken is not a page token this operation issued for this listing", exactly as the `hex::encode("v0:accounts:111111111111")` row in `tests/all/portal_session.rs:918` is pinned to answer.
5. The same holds for `/assignment/roles?account_id=111111111111&max_result=1`.
