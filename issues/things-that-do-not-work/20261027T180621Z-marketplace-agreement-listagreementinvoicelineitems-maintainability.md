Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 15c0cfc29e2643a7b1edb3bac08891ffd2eb9b9c

# The optional-instant rendering used by two page-token scopes is written out twice in state.rs

_Filed: 2026-10-27T18:06:20Z_
_Operation: ListAgreementInvoiceLineItems | Class: maintainability_

> _Note: this issue was identified against the code base at commit `15c0cfc29e2643a7b1edb3bac08891ffd2eb9b9c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two page-token scope builders in this crate's `state.rs` each define their own identical closure for turning an optional epoch-second bound into a scope part. The rendering of a bound into a scope part is one rule — it decides whether two queries hash alike — but it exists as two copies, so editing one (e.g. changing the float formatting) silently changes the token scope of one listing and not the other. Extracting the single rendering into one private helper used by both leaves every response and every existing test unchanged.

## Evidence

First copy, in `invoice_line_item_scope` — `services/.../src/state.rs:975`:
```rust
    let instant = |bound: Option<f64>| bound.map(|secs| format!("{secs:?}")).unwrap_or_default();
```
Second, byte-identical copy, in `billing_adjustment_scope` — `services/.../src/state.rs:2963`:
```rust
    let instant = |bound: Option<f64>| bound.map(|secs| format!("{secs:?}")).unwrap_or_default();
```
Both feed `query_scope` (`src/state.rs:3280-3282`) as the trailing window parts: `src/state.rs:979-987` (`invoice_line_item_scope`: `&instant(issued_after), &instant(issued_before)`) and `src/state.rs:2964-2971` (`billing_adjustment_scope`: `&instant(created_after), &instant(created_before)`), and both doc comments claim the same single rule ("Every part is length-prefixed by `query_scope`, so no value a caller supplies can imitate the delimiters and make two different listings hash alike", `src/state.rs:963-965` and `src/state.rs:2952-2954`).

Existing tests that pin the behaviour these copies produce: `tests/all/time_windows.rs:16-17` ("`ListBillingAdjustmentRequests`' `createdAfter`/`createdBefore` and `ListAgreementInvoiceLineItems`' `afterIssuedTime`/`beforeIssuedTime`"), `tests/all/billing_adjustments.rs:266-300`, `tests/all/invoice_adjustments.rs:756-796` and `tests/all/invoice_line_item_listing.rs`.

## Steps to Reproduce

1. Open `services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/src/state.rs` at line 975 and at line 2963 and compare the two `let instant = ...` closures: they are byte-identical.
2. Change the formatting in only one of them (for example to `format!("{secs}")`): the page-token scope of that one listing changes while the other keeps the old rendering, and no test fails for the divergence itself.
3. Fix: hoist the rendering into one private function in `state.rs` and call it from both `invoice_line_item_scope` and `billing_adjustment_scope`; responses and all existing tests stay unchanged.
