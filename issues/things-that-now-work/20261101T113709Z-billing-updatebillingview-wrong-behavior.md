Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9d2e170c3a479b68290b0c2532ee0534a60c9186

# UpdateBillingView writes to a view whose health status is not HEALTHY, although both sibling mutators refuse exactly that

_Filed: 2026-11-01T11:37:09Z_
_Operation: UpdateBillingView | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9d2e170c3a479b68290b0c2532ee0534a60c9186`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The two other handlers that mutate a stored billing view refuse when the view's `health_status_code` is anything other than `HEALTHY`, both quoting the modeled `BillingViewHealthStatusException` sentence. `UpdateBillingView` declares the same error (its own module doc restates it) but never reads `health_status_code`: it resolves the CUSTOM view and immediately writes `name`, `description`, `dataFilterExpression`, `view_definition_last_updated_at` and `updated_at`. So a view in `CREATING`, `UNHEALTHY` or `UPDATING` rejects a source-view change with 400 `BillingViewHealthStatusException` and simultaneously accepts a definition change (the data filter expression) with 200 — two writers of the same record disagreeing about the same gate, and the modeled error is unreachable for this operation.

## Evidence

Sibling 1 — services/service-com-amazonaws-billing-awsbilling/src/ops/associate_source_views.rs:249-263:
```
// `BillingViewHealthStatusException`: "a billing view's health status
// prevents an operation from being performed … if the billing view is in
// a state other than HEALTHY".
if target_health != crate::state::BILLING_VIEW_STATUS_HEALTHY {
    return Err(AssociateSourceViewsErrorModeled::BillingViewHealthStatusException(...
```
Sibling 2 — services/service-com-amazonaws-billing-awsbilling/src/ops/disassociate_source_views.rs:243-260: same comment, `if view.health_status_code != crate::state::BILLING_VIEW_STATUS_HEALTHY { return Err(... BillingViewHealthStatusException ...) }`.
The handler under review — services/service-com-amazonaws-billing-awsbilling/src/ops/update_billing_view.rs:197-217 goes from the lookup straight to the write, with no health branch (a `grep health_status_code` over `src/` matches only state.rs, create_billing_view.rs, associate_source_views.rs and disassociate_source_views.rs):
```
let view = state.custom_view_mut(&input.arn).ok_or_else(|| { ... })?;

let updated_at = std::time::SystemTime::now();
if let Some(name) = input.name { view.name = name; }
...
view.updated_at = Some(updated_at);
```
The model declares the error for this very operation — services/service-com-amazonaws-billing-awsbilling/src/ops/update_billing_view.rs:76-78 (generated from the model): "`BillingViewHealthStatusException` (HTTP 400) — Exception thrown when a billing view's health status prevents an operation from being performed. This may occur if the billing view is in a state other than HEALTHY." (also OPERATIONS.md:182).
The non-HEALTHY state is reachable and already exercised in-crate: services/service-com-amazonaws-billing-awsbilling/tests/all/source_view_disassociation.rs:128-138 (`set_health_status`, "the one way a view's health moves in this emulator: no modelled operation sets it") and the test `a_view_that_is_not_healthy_cannot_be_disassociated` (source_view_disassociation.rs:276-305) which loops over `["CREATING", "UNHEALTHY", "UPDATING"]` and asserts `(400, "BillingViewHealthStatusException")`. No test covers UpdateBillingView against the same states.

## Steps to Reproduce

1. Boot the emulator with a shared engine handle (as tests/all/source_view_disassociation.rs does with `emulator_with_engine`).
2. CreateBillingView(name="health", sourceViews=[<primary arn>]) and note the minted ARN.
3. Set the stored view's `health_status_code` to "UNHEALTHY" (the harness helper at tests/all/source_view_disassociation.rs:130).
4. Call DisassociateSourceViews(arn=<arn>, sourceViews=[<a held source>]) — observe 400 BillingViewHealthStatusException.
5. Call UpdateBillingView(arn=<arn>, dataFilterExpression={...}) — observe 200 and a mutated record (GetBillingView reports the new expression and a moved viewDefinitionLastUpdatedAt), where the sibling writers refuse.
