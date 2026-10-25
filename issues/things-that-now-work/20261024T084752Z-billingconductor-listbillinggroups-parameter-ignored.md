Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 1f773e23b3e0bd2b9ce8ba90d57212de869e61a4

# ListBillingGroups ignores BillingPeriod and always reports the groups' current state

_Filed: 2026-10-24T08:47:52Z_
_Operation: ListBillingGroups | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `1f773e23b3e0bd2b9ce8ba90d57212de869e61a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListBillingGroups` is documented as "A paginated call to retrieve a list of billing groups for the given billing period. If you don't provide a billing group, the current billing period is used", and its `BillingPeriod` input member is "The preferred billing period to get billing groups". The real service answers for THAT month: a billing group that did not exist in the requested period is not listed, and the membership (`Size`), `ComputationPreference`, `Status` and `AccountGrouping` reported are the ones in effect for that period.

Expected: a `ListBillingGroups` naming a billing period earlier than a group's creation does not report that group, and a period during which the group had a different pricing plan / different member accounts reports that period's values.

Actual: the handler drops `BillingPeriod` entirely. It is read nowhere in `handle` (only the model's `@pattern` is enforced ahead of the handler), the selection walks `state.billing_groups` unconditionally, and `element()` renders each group's CURRENT record. Every billing period — past, present or future — returns the same answer, including groups created after the period asked about. Unlike the sibling readers (`ListPricingPlans`, `ListPricingRules`, `ListPricingRulesAssociatedToPricingPlan`), the output shape here carries no `BillingPeriod` member, so nothing in the response even signals which period was served.

## Evidence

Spec: Smithy model `ListBillingGroups` input member `BillingPeriod` (pattern `^\d{4}-(0?[1-9]|1[012])$`) with the operation doc "retrieve a list of billing groups for the given billing period" (see OPERATIONS.md, ListBillingGroups).

Handler: `services/service-com-amazonaws-billingconductor-awsbillingconductor/src/ops/list_billing_groups.rs` — the module header states the limitation outright ("LIMITATION: `BillingPeriod` selects the month to report for. This emulator keeps no per-billing-period history of a group's membership or preferences, so every period reports the group's CURRENT state"), and in `handle` the only uses of `input` are `input.filters`, `input.next_token` and `input.max_results`; `input.billing_period` is never read. `crate::state::BillingGroupRecord` stores a single current snapshot (no per-period history) — `src/state.rs`, `BillingGroupRecord`.

Tests: no test asserts period-scoped results for this operation. `tests/all/validation_exception_message.rs:294` only asserts the `@pattern` refusal for a malformed `BillingPeriod`, and `tests/all/payer_account_authorization.rs:77` sends a `BillingPeriod` only to exercise the 403 payer gate. The same gap on a sibling operation is already accepted as reportable: open issue `20261024T045911Z-billingconductor-listaccountassociations-parameter-ignored.md` ("ListAccountAssociations ignores BillingPeriod and always reports the current associations"). This issue is the `ListBillingGroups` instance, which that issue does not cover.

## Steps to Reproduce

1. Make the emulated account the payer so the control plane is reachable (see the open payer-account issue), then `CreateBillingGroup` with Name="GroupA", a valid `ComputationPreference.PricingPlanArn` and `AccountGrouping.LinkedAccountIds=["111111111111"]`.
2. POST /list-billing-groups with `{"BillingPeriod": "2000-01"}` — a month years before the group was created.
3. Observe the response reports GroupA with its current `Size`, `Status` and `ComputationPreference`; the real service reports no billing group for a period in which none existed.
4. POST /list-billing-groups with `{"BillingPeriod": "2099-12"}` and observe the byte-identical answer, confirming the member is dropped rather than applied.
