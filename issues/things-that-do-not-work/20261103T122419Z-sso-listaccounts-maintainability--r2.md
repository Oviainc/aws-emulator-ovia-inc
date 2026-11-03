Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 30be2c6aa9a038e82cb0ad1355d2adc7b9e702f5

# The refused-cursor refusal (class + wording) is written twice in the handlers, though state.rs owns every shared refusal so none is written twice

_Filed: 2026-11-03T12:24:19Z_
_Operation: ListAccounts | Class: maintainability_

> _Note: this issue was identified against the code base at commit `30be2c6aa9a038e82cb0ad1355d2adc7b9e702f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` is documented as the home of every rule the portal handlers share — explicitly including the WORDING of a refusal, which is why `unauthorized_session()` exists and why every handler returns that one value instead of a message of its own. The cursor refusal breaks that arrangement: `state::page_of` hands back `state::InvalidCursor` (a type the state module owns), and each of the two paginated handlers then decides for itself that an `InvalidCursor` is an `InvalidRequestException` and spells the same sentence out in full. The rule "a nextToken this listing did not issue is a 400 InvalidRequestException worded thus" therefore lives in two places in src/, and the two copies can drift exactly the way `unauthorized_session` was centralized to prevent: reword or reclassify one listing's refusal and the other still answers the old one, with nothing in the crate tying them together. The crate's own test file states there is only ONE such wording and names both source copies as its definition, so the duplication is already recognised as a single rule.

Fix shape: give `crate::state` the cursor refusal the way it already owns `INVALID_SESSION_MESSAGE` / `unauthorized_session()` (e.g. an `INVALID_CURSOR_MESSAGE` constant, or a helper returning the `InvalidRequestException`), and have both handlers map `InvalidCursor` through it. Every status, `__type`, message byte and existing test stays exactly as it is — `tests/all/page_token_spelling.rs` asserts the same 400/`InvalidRequestException`/`REFUSAL` text for both listings either way.

## Evidence

The principle the state module states for itself — services/service-com-amazonaws-sso-swbportalservice/src/state.rs:12-17:
```
//! This module also owns the rules every handler shares, so none is written twice:
//! resolving a presented access token to a usable session
//! ([`ComAmazonawsSsoSwbportalserviceState::session`]), wording the refusal when it
//! resolves to nothing ([`unauthorized_session`], and [`unauthorized_response`] for the
//! gate in [`crate::service`] that runs before any handler), and cutting a sorted
//! listing into a page with an opaque cursor ([`page_of`]).
```
and how that is enforced for the OTHER refusal — src/state.rs:61-70:
```rust
/// Every handler returns THIS value — wrapped in its own operation's error enum — rather
/// than a message of its own, so the gate cannot drift per operation and nothing about
/// the request or the emulator's wiring leaks into it.
pub fn unauthorized_session() -> crate::ops::types::UnauthorizedException {
    crate::ops::types::UnauthorizedException {
        message: Some(INVALID_SESSION_MESSAGE.to_string()),
    }
}
```

Copy 1 — src/ops/list_accounts.rs:192-205:
```rust
        let page = crate::state::page_of(
            assigned,
            &crate::state::cursor_scope("accounts", &input.access_token),
            input.next_token.as_deref(),
            input.max_results,
        )
        .map_err(|_| {
            ListAccountsErrorModeled::InvalidRequestException(InvalidRequestException {
                message: Some(
                    "nextToken is not a page token this operation issued for this listing"
                        .to_string(),
                ),
            })
        })?;
```
Copy 2 — src/ops/list_account_roles.rs:200-216:
```rust
        let page = crate::state::page_of(
            assigned,
            &crate::state::cursor_scope(
                &format!("roles:{}", account.account_id),
                &input.access_token,
            ),
            input.next_token.as_deref(),
            input.max_results,
        )
        .map_err(|_| {
            ListAccountRolesErrorModeled::InvalidRequestException(InvalidRequestException {
                message: Some(
                    "nextToken is not a page token this operation issued for this listing"
                        .to_string(),
                ),
            })
        })?;
```
That these are one rule, not two, is stated by the suite that pins them — tests/all/page_token_spelling.rs:34-36:
```rust
/// The one wording both listings refuse a token they did not issue with
/// (`src/ops/list_accounts.rs`, `src/ops/list_account_roles.rs`).
const REFUSAL: &str = "nextToken is not a page token this operation issued for this listing";
```
and the state module owns the type the two copies are reacting to — src/state.rs:336-338:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidCursor;
```

Test coverage that must stay green and unchanged: tests/all/page_token_spelling.rs:304-346 (`every_paginated_listing_refuses_a_respelt_page_token_and_resumes_the_issued_one`) asserts `(StatusCode::BAD_REQUEST, "InvalidRequestException")` and `message == REFUSAL` for BOTH listings (`listings()` at tests/all/page_token_spelling.rs:131), and tests/all/page_bound_clamp.rs:256-341 plus tests/all/portal_session.rs:307 walk the success paths of the same two handlers.

## Steps to Reproduce

1. Read services/service-com-amazonaws-sso-swbportalservice/src/state.rs:12-17 — the state module declares it owns the rules every handler shares, 'so none is written twice', naming refusal wording as one of them (src/state.rs:61-70 is that pattern applied to the unauthorized refusal).
2. Read src/ops/list_accounts.rs:198-205 and src/ops/list_account_roles.rs:209-216: both independently choose `InvalidRequestException` for a `state::InvalidCursor` and both spell the identical sentence 'nextToken is not a page token this operation issued for this listing'. Neither calls a shared helper; `grep -rn "not a page token" services/service-com-amazonaws-sso-swbportalservice/src` returns exactly these two sites and none in src/state.rs.
3. Reword or reclassify one copy (e.g. change list_account_roles.rs:211-214's sentence): the crate still compiles and ListAccounts keeps answering the old wording, so the two paginated operations now refuse the same class of token differently — the per-operation drift src/state.rs:61-65 centralizes `unauthorized_session()` to make impossible.
4. Confirm the rule is understood as single: tests/all/page_token_spelling.rs:34-36 defines one `REFUSAL` constant and cites both handler files as its definition, and tests/all/page_token_spelling.rs:323-335 asserts that one value for both listings.
