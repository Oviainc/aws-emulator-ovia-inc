Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f60279ee1b26e62c04ba7bfaecc753f1127e63e1

# Fold the duplicated "pair each selected row with its stored position from the token offset" step into the shared pagination rule

_Filed: 2026-10-29T08:57:18Z_
_Operation: ListAccountActivities | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f60279ee1b26e62c04ba7bfaecc753f1127e63e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` deliberately states this service's pagination rule ONCE — the token codec (`encode_page_token`/`decode_page_token`), the page-size resolution (`page_size`) and the page cut (`cut_page`) all live together in `state.rs`, and both paginated handlers call them. But the fourth part of that rule — "start at the token's offset, keep the rows the request's predicate selects, and carry each kept row's position in the STORED order" — is NOT in `state.rs`. It is hand-written twice, once in each paginated handler, in two different forms: an iterator chain in `list_account_activities.rs` and an explicit accumulate-into-`Vec` loop in `get_free_tier_usage.rs`.

That step is a documented PRECONDITION of `cut_page`, which nothing enforces: `cut_page` derives the next token from `page.last()`'s stored position, so if either copy drifted (for example by calling `.skip(offset)` after `.filter(..)`, or by enumerating the filtered view instead of the stored collection) the token that handler issues would become an offset into the FILTERED view, and pages would silently skip or repeat rows for any request that carries a filter. The two copies can drift independently because they share no code and look nothing alike.

The fix is to state this step once beside `cut_page` in `state.rs` (a helper taking the stored slice, the decoded offset and a predicate, returning `Vec<(usize, &T)>`) and have both handlers call it, exactly as they already both call `page_size` and `cut_page`. Every response and every existing test stays byte-identical: the computation is the same, only its single statement moves.

## Evidence

`state.rs` says the pagination rule is stated once, and states the stored-position pairing as a requirement on its caller — services/service-com-amazonaws-freetier-awsfreetierservice/src/state.rs:499-511:
```rust
/// [`encode_page_token`]/[`decode_page_token`] and [`page_size`], stated here
/// ONCE so both paginated operations page by the same rule: a page reports at
/// most `page_size` of the selected rows, in the order they were selected, and
/// a token is issued only while a selected row is still unreported — the
/// `@paginated` contract is that an absent token ends the iteration, so an
/// exactly-filled last page carries none.
///
/// `rows` pairs each selected item with its position in the STORED order, so
/// the token the page issues is an offset into the stored collection and not
/// into the filtered view: changing the filter between pages cannot make a page
/// resume in the middle of a row it already reported.
pub fn cut_page<'rows, T>(
```

Copy 1 — src/ops/list_account_activities.rs:209-220 (iterator chain):
```rust
        // Catalog position kept beside each row: the token is an offset into the
        // catalog, not into the filtered view, so changing the filter between
        // pages cannot make a page resume in the middle of a row it already
        // reported.
        let selected: Vec<(usize, ActivitySummary)> = store
            .activities_for(crate::state::caller_account_id())
            .iter()
            .enumerate()
            .skip(offset)
            .filter(|(_, activity)| {
                wanted.is_empty() || wanted.contains(&activity.status)
            })
```

Copy 2 — src/ops/get_free_tier_usage.rs:190-200 (explicit loop, same rule):
```rust
        // Position in the stored order kept beside each selected row, so the
        // token is an offset into the records and not into the filtered view.
        let mut selected: Vec<(usize, &crate::state::UsageRecord)> = Vec::new();
        for (position, record) in records.iter().enumerate().skip(offset) {
            let keep = filter
                .as_ref()
                .is_none_or(|filter| matches_expression(record, filter));
            if keep {
                selected.push((position, record));
            }
        }
```

Both handlers then hand the result to the one shared cut (list_account_activities.rs:240 and get_free_tier_usage.rs:204): `crate::state::cut_page(PAGE_KIND, &selected, page_size)`.

Test coverage that must stay green and unchanged after the fold:
* `tests/all/shared_rules.rs:86` `both_paginated_operations_page_by_one_rule` — runs the same page-size table (1..=total+1, including the exact fit) against BOTH `ListAccountActivities` and `GetFreeTierUsage` and asserts the pages reassemble the collection in stored order exactly once.
* `tests/all/free_tier.rs:539` `activity_pages_reassemble_the_catalog_at_every_page_size` and `tests/all/free_tier.rs:1199` `usage_pages_reassemble_the_records_at_every_page_size` ("with and without a filter").
* `tests/all/free_tier.rs:595` `an_unissued_activity_token_is_refused` and `tests/all/free_tier.rs:1287` `an_unissued_usage_token_is_refused`.

## Steps to Reproduce

1. Open services/service-com-amazonaws-freetier-awsfreetierservice/src/state.rs:495-525 and read `cut_page`'s doc: the token it issues is derived from the stored position its caller paired with each selected row, and the doc says the pagination rule is stated there ONCE.
2. Open src/ops/list_account_activities.rs:209-235 — the handler hand-writes `.iter().enumerate().skip(offset).filter(..)` to satisfy that precondition.
3. Open src/ops/get_free_tier_usage.rs:190-200 — the same precondition is satisfied again by a different hand-written form (a `for` loop pushing `(position, record)`).
4. Confirm no shared helper exists: `grep -n "enumerate()" services/service-com-amazonaws-freetier-awsfreetierservice/src` returns only those two handler sites, and `state.rs` carries no selection helper beside `cut_page`.
5. Observe the drift risk: change either copy to `.filter(..).enumerate()` (or to enumerate after filtering) and that operation's `nextToken` becomes an offset into the filtered view, so a filtered walk skips rows — while the other operation keeps working, since the copies are independent.
