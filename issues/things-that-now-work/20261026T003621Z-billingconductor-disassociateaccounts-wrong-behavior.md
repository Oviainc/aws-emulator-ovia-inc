Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 232a76c0adfd6241e7fe920f3ef518cb863b87b4

# DisassociateAccounts moves the billing group's generation start forward without superseding the generation it replaces

_Filed: 2026-10-26T00:36:21Z_
_Operation: DisassociateAccounts | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `232a76c0adfd6241e7fe920f3ef518cb863b87b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Identical defect to the one filed for `AssociateAccounts`, in the other half of the pair. `DisassociateAccounts` mutates the stored billing group through `billing_groups.get_mut(..)` and assigns `group.last_modified_time_millis = now`, bypassing `ComAmazonawsBillingconductorAwsbillingconductorState::put_billing_group` — the only writer that freezes the generation a write supersedes.

Because `BillingGroupRecord::version_start_billing_period()` is derived from `last_modified_time_millis`, the call moves the current generation's start period forward to the period of the disassociation while the generation it displaced is never recorded in `superseded_versions`. `BillingGroupRecord::effective_in` then finds no generation starting at or before the displaced period and falls back to `versions.first()`, so `ListBillingGroups` scoped to that period reports an older generation's Name / Description / ComputationPreference.PricingPlanArn / AutoAssociate instead of the values the group really carried then. The group's own persisted history is lost by an operation whose documented job is only to remove accounts.

## Evidence

The path that skips the helper, `services/service-com-amazonaws-billingconductor-awsbillingconductor/src/ops/disassociate_accounts.rs:242-250`:

        let now = crate::state::now_millis();
        if let Some(group) = state.billing_groups.get_mut(&group_arn) {
            if !input.account_ids.is_empty() {
                group
                    .linked_account_ids
                    .retain(|account_id| !input.account_ids.contains(account_id));
                group.last_modified_time_millis = now;
            }
        }

the sibling in the same crate that does use it, `src/ops/update_billing_group.rs:359-363`:

            // Through the state method that keeps the generation this write supersedes,
            // so a read scoped to a billing period the previous generation was in force
            // in still reports the group as it stood then
            // (`crate::state::ComAmazonawsBillingconductorAwsbillingconductorState::put_billing_group`).
            state.put_billing_group(updated.clone());

and the rule it bypasses, `src/state.rs:2055-2063` / `src/state.rs:857-859`:

    pub fn put_billing_group(&mut self, mut updated: BillingGroupRecord) {
        ...
            .filter(|previous| {
                previous.version_start_billing_period() != updated.version_start_billing_period()
            })
            .map(BillingGroupRecord::current_version);

    pub fn version_start_billing_period(&self) -> String {
        billing_period_at(self.last_modified_time_millis)
    }

Test coverage: `tests/all/disassociate_accounts.rs` asserts the membership and `Size` (`tests/all/disassociate_accounts.rs:175`), and `tests/all/billing_group_billing_periods.rs:506` builds its multi-generation fixtures with the direct-state helpers `created_in` / `generation_started_in` (lines 191-216) rather than through this operation, so this path is unpinned.

## Steps to Reproduce

1. Create a billing group with two linked accounts; treat the create as having happened in billing period M1.
2. In a later billing period M2, call UpdateBillingGroup with a new Description. put_billing_group freezes the M1 generation; ListBillingGroups with BillingPeriod = M2 reports the new Description.
3. In a still later billing period M3, call DisassociateAccounts for one of the accounts.
4. Call ListBillingGroups with BillingPeriod = M2 again: the M2 generation is gone and the M1 Description is reported, because disassociate_accounts.rs:248 moved last_modified_time_millis into M3 without superseding the M2 generation.
