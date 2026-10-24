Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4af528f360b03ab2b05e9ad5a75be6ebefdc29e9

# DeleteBillingGroup erases the group's current-billing-period occupancy, so a same-period re-create is accepted instead of refused (BILLING_GROUP_ALREADY_EXIST_IN_CURRENT_BILLING_PERIOD)

_Filed: 2026-10-24T21:47:01Z_
_Operation: DeleteBillingGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4af528f360b03ab2b05e9ad5a75be6ebefdc29e9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A billing group is a billing-period-scoped resource: it bills a consolidated family for a month, so the month a group existed in keeps a record of it even after the group is deleted. The model carries a refusal keyed on exactly that: ValidationExceptionReason enumerates BILLING_GROUP_ALREADY_EXIST_IN_CURRENT_BILLING_PERIOD, and the only operations that could raise it are the ones that create or re-create a group inside a period one already occupied (CreateBillingGroup declares ValidationException 400).

Expected: deleting a billing group does not erase its existence for the billing period it billed in. A CreateBillingGroup made in the SAME billing period that re-uses the deleted group's name (or re-claims the linked accounts the deleted group held for that period) is refused HTTP 400 ValidationException with Reason BILLING_GROUP_ALREADY_EXIST_IN_CURRENT_BILLING_PERIOD; only from the next billing period onward is the name/occupancy free.

Actual: crate::state::remove_billing_group drops the record, its billing_group_names index entry and every account_associations entry for it unconditionally, with no record of the period the group existed in. The very next CreateBillingGroup in the same month re-uses the name and the accounts and answers HTTP 200 with a fresh ARN; nothing in the service can ever produce BILLING_GROUP_ALREADY_EXIST_IN_CURRENT_BILLING_PERIOD. The same erasure is why ListBillingGroups/ListAccountAssociations report the month as if the deleted group never billed in it (the existing ListBillingGroups BillingPeriod issue is the read-side symptom of the same missing per-period history; this issue is about the write that discards it).

## Evidence

Spec: ValidationExceptionReason (smithy_query query="shape", shape="ValidationExceptionReason") enumerates BILLING_GROUP_ALREADY_EXIST_IN_CURRENT_BILLING_PERIOD — a reason that names a billing group that already exists IN THE CURRENT BILLING PERIOD, which only has meaning if a group deleted during that period still occupies it. CreateBillingGroup lists ValidationException (HTTP 400) among its errors; DeleteBillingGroup lists ValidationException as its only client-input error.

Handler: services/service-com-amazonaws-billingconductor-awsbillingconductor/src/ops/delete_billing_group.rs lines ~155-173 — the handler normalises the ARN and calls state.remove_billing_group(&group_arn), whose only refusal is INVALID_BILLING_GROUP for a group that is not stored. src/state.rs remove_billing_group (lines ~983-1026) removes billing_groups[arn], billing_group_names[name], the group's custom line items and its account_associations entries, keeping no tombstone and no billing period. src/ops/create_billing_group.rs lines ~219-227 decides the name conflict purely from state.billing_group_names, so a freed name is immediately re-usable in the same month, and the account check (conflicting_association) likewise sees the released accounts as unowned.

Tests: tests/all/billing_group_core_loop.rs lines ~1244-1294 delete groups and assert the accounts become UNMONITORED and a second delete is refused — no test re-creates a billing group under a deleted group's name or re-claims its accounts in the same billing period, so nothing pins either behaviour. (By contrast tests/all/pricing_plan_delete.rs:379 and tests/all/pricing_rule_lifecycle.rs:705 DO pin name re-use after delete for plans and rules, whose names the model documents as plain uniqueness with no period scoping.) The existing open DeleteBillingGroup issue (…-deletebillinggroup-validation.md) is about CANNOT_DELETE_AUTO_ASSOCIATE_BILLING_GROUP, a different rule.

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable (otherwise every gated operation answers 403).
2. POST /create-pricing-plan with {"Name":"Plan1"} and keep the returned Arn.
3. POST /create-billing-group with {"Name":"GroupA","AccountGrouping":{"LinkedAccountIds":["111122223333"]},"ComputationPreference":{"PricingPlanArn":"<plan arn>"},"PrimaryAccountId":"111122223333"} and keep the ARN.
4. POST /delete-billing-group with {"Arn":"<group arn>"} — HTTP 200.
5. Immediately (same billing period) POST /create-billing-group again with the SAME Name "GroupA" and the same LinkedAccountIds.
6. Observe HTTP 200 and a brand-new ARN, where the real service refuses the request for the current billing period with 400 ValidationException / Reason BILLING_GROUP_ALREADY_EXIST_IN_CURRENT_BILLING_PERIOD.
7. Also POST /list-billing-groups with {"BillingPeriod":"<the current period>"} and observe only the new group — the deleted group that billed in that same period is reported nowhere.
