Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# The "an accountId named twice names one account" rule is hand-rolled four times, in three different ways

_Filed: 2026-10-25T06:09:07Z_
_Operation: BatchDeleteTaxRegistration | Class: maintainability_

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation of this crate that takes an `accountIds` list implements the SAME rule — "an id named more than once names ONE account and ONE action on it, so act on it once" — and each one implements it again by hand, with a different data structure each time:

* `BatchDeleteTaxRegistration` and `BatchPutTaxRegistration` each build a `BTreeSet<&str>` named `addressed` and `continue` on a repeat (request order preserved);
* `BatchGetTaxExemptions` builds a `Vec<&str>` named `named` and uses a linear `contains` scan (request order preserved);
* `state::store_exemptions`, the writer `PutTaxExemption` calls, collects `BTreeSet<&str>` named `distinct` (request order LOST — the accounts are then walked in sorted order).

So one rule has four copies and the copies have already drifted in two ways: the membership test (ordered set vs. O(n²) vector scan) and the iteration order of the surviving ids. Nothing a caller sees changes today — the batch error rows are built in request order in the two handlers that build them, and `store_exemptions`' per-account writes and its distinct-additions count are order-independent — which is exactly why this is a maintainability defect and not a behavioral one: the next change to what a repeated id means (or to the comment that explains it, which is restated in all three handlers) has four places to touch, and one of them already deviates.

Folding it into a single helper beside the other shared decisions in `state.rs` (one `distinct account ids, first-seen order` function, next to `resolve_account_id`, which is already the one place an addressed account is normalised) would leave every response and every existing test byte-identical, and would let the rule and its comment be stated once — the same consolidation the crate already made for the page-build rule (`state::build_page`) and for entry validation (`state::check_tax_registration_entry`).

## Evidence

Copy 1 — `src/ops/batch_delete_tax_registration.rs:161`:
```
        let mut addressed: ::std::collections::BTreeSet<&str> =
            ::std::collections::BTreeSet::new();
        for account_id in &input.account_ids {
            // `accountIds` is "List of unique account identifiers", and the
            // registration of every account the request names is removed once,
            // so an id named twice names the SAME account and the SAME single
            // deletion
            if !addressed.insert(account_id.as_str()) {
                continue;
            }
```

Copy 2 — `src/ops/batch_put_tax_registration.rs:226` (the same two statements, the same variable name, the same comment restated):
```
        let mut addressed: ::std::collections::BTreeSet<&str> =
            ::std::collections::BTreeSet::new();
        for account_id in &input.account_ids {
            // `accountIds` is "List of unique account identifiers", and one
            // entry is written to every account the request names, so an id
            // named twice names the SAME write: it is performed once rather
            // than a second time against the record this very call just wrote.
            if !addressed.insert(account_id.as_str()) {
                continue;
            }
```

Copy 3 — `src/ops/batch_get_tax_exemptions.rs:162` (same rule, different structure and a linear scan):
```
        let mut named: Vec<&str> = Vec::new();
        for account_id in &input.account_ids {
            if !named.contains(&account_id.as_str()) {
                named.push(account_id.as_str());
            }
        }
```

Copy 4 — `src/state.rs:540`, inside `store_registration`'s sibling `store_exemptions` (the writer `src/ops/put_tax_exemption.rs:224` calls), which drops the request order the other three keep:
```
        let distinct: BTreeSet<&str> = account_ids.iter().map(String::as_str).collect();
```

That the four are one rule is stated once, for all four operations, by the test that pins it — `tests/all/repeated_account_ids.rs:5`:
```
//! Every operation of this service that takes `accountIds` documents it as a
//! "List of unique account identifiers", so an id named more than once names ONE
//! account and ONE action on it. The rule under test: such a request answers
//! exactly as the same request with the repeats collapsed does — the same
//! response and the same state afterwards
```
and `repeated_account_ids.rs:17` names the four operations it crosses: "(`BatchDeleteTaxRegistration`, `BatchPutTaxRegistration`, `BatchGetTaxExemptions`, `PutTaxExemption`)". Those rows, plus `tests/all/batch_tax_registration.rs` and `tests/all/tax_exemptions.rs`, are the coverage a de-duplication must leave unchanged.

## Steps to Reproduce

1. Open src/ops/batch_delete_tax_registration.rs:161-173, src/ops/batch_put_tax_registration.rs:226-235, src/ops/batch_get_tax_exemptions.rs:162-167 and src/state.rs:540 side by side: each one re-implements "collapse repeated accountIds, act once per distinct id".
2. Compare the four: two use `BTreeSet::insert` + `continue`, one uses `Vec::contains` + `push`, and the fourth uses `BTreeSet` `collect` — so the membership test and (for the fourth) the surviving iteration order already differ between copies of one rule.
3. Read tests/all/repeated_account_ids.rs:5-18, which states the rule ONCE and crosses it over all four operations, confirming it is a single rule with four implementations rather than four separate rules.
4. Confirm no response is at stake: the batch handlers build their error rows while walking `input.account_ids` in order, and store_exemptions' writes plus its distinct-additions count are order-independent — so a single first-seen-order helper keeps every existing response and every assertion in tests/all/repeated_account_ids.rs, tests/all/batch_tax_registration.rs and tests/all/tax_exemptions.rs unchanged.
