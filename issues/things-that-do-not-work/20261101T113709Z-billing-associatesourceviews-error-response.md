Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 9d2e170c3a479b68290b0c2532ee0534a60c9186

# AssociateSourceViews skips the account-boundary gate its own counterpart applies, answering 404 instead of 403 for a foreign-account ARN

_Filed: 2026-11-01T11:37:09Z_
_Operation: AssociateSourceViews | Class: error-response_

> _Note: this issue was identified against the code base at commit `9d2e170c3a479b68290b0c2532ee0534a60c9186`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other billing-view operation of this crate that identifies a view by `arn` refuses a well-formed ARN whose embedded account id is not the caller's own with the modeled `AccessDeniedException` (403) BEFORE the store is consulted — `GetBillingView`, `UpdateBillingView`, `DeleteBillingView`, `ListBillingViews` (on each `arns` entry) and `DisassociateSourceViews` all call `crate::state::is_caller_account_arn` as their first act. `AssociateSourceViews` — whose input shape is identical to `DisassociateSourceViews`' (`arn` + `sourceViews`) and which also declares `AccessDeniedException` (HTTP 403) — never calls it: it locks the store and does a bare key lookup, so a cross-account billing-view ARN is answered with `ResourceNotFoundException` (404) whose message echoes the foreign ARN. That is the exact disclosure channel the sibling handlers' comments say they exist to close (a caller can tell "exists in that account" from "does not" by the status), and it makes the two halves of one source-view association answer two different codes for the same ARN.

## Evidence

Sibling that applies the gate — services/service-com-amazonaws-billing-awsbilling/src/ops/disassociate_source_views.rs:208-222:
```
// The account boundary comes first, as it does for every operation that
// identifies a billing view by ARN: a well-formed ARN outside the
// caller's own account is refused before the store is consulted, so the
// answer never depends on what another account holds.
if !crate::state::is_caller_account_arn(&input.arn) {
    return Err(DisassociateSourceViewsErrorModeled::AccessDeniedException(
        crate::state::cross_account_access_denied(Self::NAME, &input.arn, &*self.engine, ctx),
    )
    .into());
}
```
The handler under review — services/service-com-amazonaws-billing-awsbilling/src/ops/associate_source_views.rs:193-203 (no boundary check anywhere in the file; `grep is_caller_account_arn` lists update/get/disassociate/delete/list_billing_views only):
```
let _ = ctx;
let state = crate::state::service_state(&*self.engine).ok_or_else(...)?;
let mut state = state.lock();

let Some(target) = state.billing_views.get(&input.arn) else {
    return Err(not_found(&input.arn, "arn"));
};
```
The model gives this operation the 403 it never answers — OPERATIONS.md:32 for AssociateSourceViews: "**Errors:** `AccessDeniedException (HTTP 403)`, …".
Test coverage proves the asymmetry is untested here: services/service-com-amazonaws-billing-awsbilling/tests/all/source_view_disassociation.rs:353-358 pins the sibling —
```
(
    "an ARN in another account",
    "arn:aws:billing::111122223333:billingview/primary".to_string(),
    403,
    "AccessDeniedException",
),
```
while services/service-com-amazonaws-billing-awsbilling/tests/all/rejection_fidelity.rs:334-341 claims to cover the whole class but omits this operation: "// Every implemented operation that identifies a billing view by ARN; the\n// last one reaches the ARN through the `arns` filter." followed by a 4-row table of GetBillingView/UpdateBillingView/DeleteBillingView/ListBillingViews.

## Steps to Reproduce

1. Boot the emulator and point an awsJson1_0 `billing` client at it.
2. Call DisassociateSourceViews(arn="arn:aws:billing::111122223333:billingview/primary", sourceViews=["arn:aws:billing::000000000000:billingview/primary"]) — observe 403 AccessDeniedException (pinned by tests/all/source_view_disassociation.rs:353).
3. Call AssociateSourceViews with the same foreign `arn` and the same `sourceViews` — observe 404 ResourceNotFoundException whose message contains the foreign ARN, instead of the modeled 403 AccessDeniedException.
