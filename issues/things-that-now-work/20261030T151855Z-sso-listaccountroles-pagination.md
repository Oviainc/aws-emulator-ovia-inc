Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 4ac27352e7b3d7cddd4c7ab08e294db28bfa593a

# page_of turns a negative max_results into the LARGEST page instead of the floor its own comment states

_Filed: 2026-10-30T15:18:55Z_
_Operation: ListAccountRoles | Class: pagination_

> _Note: this issue was identified against the code base at commit `4ac27352e7b3d7cddd4c7ab08e294db28bfa593a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::page_of` is the one shared page-cutter both listing handlers call (`ListAccountRoles` at `src/ops/list_account_roles.rs:200-208`, `ListAccounts` at `src/ops/list_accounts.rs:192-197`), and it documents its own treatment of an out-of-range `max_results`: the clamp it applies is "only a floor for a direct, non-HTTP caller". The code does not do that. `max_results` is an `Option<i32>`; the chain `.and_then(|requested| usize::try_from(requested).ok())` yields `None` for every NEGATIVE value, because `usize::try_from(-1)` is an error, so `unwrap_or(MAX_PAGE_SIZE)` replaces it with 100 BEFORE `.clamp(1, MAX_PAGE_SIZE)` ever sees it. A negative request therefore produces the biggest page the service gives (100 items), the opposite end of the clamp from the floor the comment promises, and it is silently indistinguishable from `max_results: None` — which the same doc defines as "defaulting to MAX_PAGE_SIZE when the caller names none". A caller that named a value is treated as one that named none.

The zero case shows the intended rule working: `usize::try_from(0)` succeeds, so `0` really is floored to 1 by the clamp. Only the sign-losing conversion escapes it, so the module's stated rule holds for one half of the out-of-range domain and is inverted for the other. `state.rs` exists precisely to hold "the rules every handler shares, so none is written twice" (`src/state.rs:12-17`), so the defect is in the one place every listing inherits it from.

Scope note, stated honestly: from the wire this path is unreachable — the modeled `MaxResultType` range (1..100) is enforced by the boundary validator before any handler runs, which `tests/all/portal_session.rs:811-820` pins (`"-1"` answers 400 `InvalidRequestException`). `page_of` is nevertheless `pub` in the `pub mod state` this crate exports (the same surface the tests seed the store through), so a direct, non-HTTP caller — exactly the caller the comment scopes itself to — gets 100 items where the comment says it gets 1. Hence LOW: no response a client can observe changes today.

## Evidence

The rule the helper states for itself (`services/service-com-amazonaws-sso-swbportalservice/src/state.rs:357-369`):
```
/// Cut `sorted` — a listing of `(sort key, item)` pairs in ascending key order — into
/// the page a request asked for.
///
/// `next_token` resumes strictly after the key it carries, so a continuation never
/// repeats or skips an item; `max_results` bounds the page, defaulting to
/// [`MAX_PAGE_SIZE`] when the caller names none; and the token comes back only when
/// items were left over, so the last page carries none.
///
/// A `max_results` OUTSIDE the modeled `MaxResultType` range (1..100) never reaches
/// here from the wire: the boundary validator refuses it before the handler runs, and
/// this service words that refusal as `InvalidRequestException` (the 400 rows in
/// `tests/all/portal_session.rs` pin it). The clamp below is therefore only a floor for
/// a direct, non-HTTP caller, not a silent reshaping of a request a client sent.
```

The code under that comment (`src/state.rs:380-383`):
```rust
    let limit = max_results
        .and_then(|requested| usize::try_from(requested).ok())
        .unwrap_or(MAX_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE);
```
`usize::try_from(-1_i32)` is `Err`, so `.ok()` is `None` and `unwrap_or(MAX_PAGE_SIZE)` substitutes 100; `.clamp(1, 100)` leaves 100 untouched. The "floor" the comment names is never applied to a negative value — it becomes the CEILING, and the named value is folded into the "caller names none" case.

Where `MAX_PAGE_SIZE` comes from (`src/state.rs:156-159`):
```rust
/// The largest page any listing returns, which is also the largest `maxResults` the
/// model's `MaxResultType` range (1..100) admits — so it doubles as the page size for
/// a request that asks for no particular one.
pub const MAX_PAGE_SIZE: usize = 100;
```

The caller this is filed against (`src/ops/list_account_roles.rs:200-208`):
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
```
`ListAccounts` inherits the same helper (`src/ops/list_accounts.rs:192-197`).

Test coverage: `tests/all/portal_session.rs:811-820` sweeps `max_result` over the wire — `("1", Served)`, `("100", Served)`, `("0", Refused("InvalidRequestException"))`, `("101", Refused(...))`, `("-1", Refused(...))`, `("not-a-number", RefusedByTheCodec)` — which pins that no negative value reaches `page_of` from HTTP, and is why the helper's own negative-value behaviour is unpinned: no test calls `page_of` (or the pub `state` API) directly with a negative `max_results`.

## Steps to Reproduce

1. Seed the store as `tests/all/portal_session.rs` does: `put_account` for four accounts and `put_session("tok-live", …, [all four ids], ttl)`.
2. Call the shared helper the way both listing handlers do, with a negative bound: `crate::state::page_of(items, &crate::state::cursor_scope("accounts", "tok-live"), None, Some(-1))` (items = the four `(account_id, AccountInfo)` pairs; `page_of`, `cursor_scope` and `ComAmazonawsSsoSwbportalserviceState` are all public, as `tests/all/portal_session.rs:21-24` shows).
3. Observed: `page.items.len() == 4` and the limit used is `MAX_PAGE_SIZE` (100) — the request for a negative page size is served the LARGEST page, identical to passing `None`.
4. Expected per the helper's own doc at `src/state.rs:365-369` ("The clamp below is therefore only a floor for a direct, non-HTTP caller"), and per the `0` case which does hit `.clamp(1, …)`: one item, the floor.
5. Confirm the two halves disagree by repeating with `Some(0)`: `usize::try_from(0)` succeeds, the clamp floors it, and exactly 1 item comes back.
