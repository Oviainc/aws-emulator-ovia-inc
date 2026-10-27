Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 232a76c0adfd6241e7fe920f3ef518cb863b87b4

# AssociateAccounts moves the billing group's generation start forward without superseding the generation it replaces, corrupting period-scoped reads

_Filed: 2026-10-26T00:36:21Z_
_Operation: AssociateAccounts | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `232a76c0adfd6241e7fe920f3ef518cb863b87b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A billing group keeps per-billing-period generations: `BillingGroupRecord::version_start_billing_period()` derives the period the CURRENT generation starts in from `last_modified_time_millis`, and `ComAmazonawsBillingconductorAwsbillingconductorState::put_billing_group` is the one writer that freezes the generation a write supersedes (it compares the previous record's `version_start_billing_period()` with the new one and pushes the old generation into `superseded_versions`).

`AssociateAccounts` does not write through that helper. It reaches into the map with `billing_groups.get_mut(..)` and assigns `group.last_modified_time_millis = now` directly, so the current generation's start period jumps to the period this call is made in while the generation it displaced is never recorded. `BillingGroupRecord::effective_in` then has no generation whose start period is at or before the displaced period and falls back to `versions.first()`, so a `ListBillingGroups` read scoped to a billing period in which the group really carried a different Name / Description / ComputationPreference.PricingPlanArn / AutoAssociate reports the values of an OLDER generation instead.

Concretely: create a group in month M1 (name "A"), `UpdateBillingGroup` it to name "B" in M2 (this supersedes the M1 generation through `put_billing_group`, so M2 correctly reports "B"), then call `AssociateAccounts` on it in M3. The M2 generation is destroyed — `last_modified_time_millis` now falls in M3 and nothing was pushed to `superseded_versions` — and `ListBillingGroups` with `BillingPeriod` = M2 reports name "A" again, i.e. the group as it stood two generations ago. The persisted history a later caller must still be able to read is silently lost by an operation whose own comment says the membership change is all it does.

The same bypass is in `DisassociateAccounts` (filed separately).

## Evidence

The helper that owns the rule, `services/service-com-amazonaws-billingconductor-awsbillingconductor/src/state.rs:2040-2063`:

    /// Store `updated` as the billing group's CURRENT generation, keeping the generation
    /// it replaces when the write takes effect in a DIFFERENT billing period.
    ...
    pub fn put_billing_group(&mut self, mut updated: BillingGroupRecord) {
        let arn = updated.arn.clone();
        let superseded = self
            .billing_groups
            .get(&arn)
            .filter(|previous| {
                previous.version_start_billing_period() != updated.version_start_billing_period()
            })
            .map(BillingGroupRecord::current_version);

what the generation start is derived from, `state.rs:851-859`:

    /// The billing period this group's CURRENT generation is in effect FROM.
    /// A write takes effect in the period it is made in, and `LastModifiedTime` is the
    /// instant of the write that left the group as it stands, so the period that instant
    /// falls in is the one the current generation starts in.
    pub fn version_start_billing_period(&self) -> String {
        billing_period_at(self.last_modified_time_millis)
    }

the sibling that DOES use it, `src/ops/update_billing_group.rs:359-363`:

            // Through the state method that keeps the generation this write supersedes,
            // so a read scoped to a billing period the previous generation was in force
            // in still reports the group as it stood then
            // (`crate::state::ComAmazonawsBillingconductorAwsbillingconductorState::put_billing_group`).
            state.put_billing_group(updated.clone());

and the path that skips it, `src/ops/associate_accounts.rs:258-264`:

        let now = crate::state::now_millis();
        if let Some(group) = state.billing_groups.get_mut(&group_arn) {
            if !to_add.is_empty() {
                group.linked_account_ids.extend(to_add.iter().cloned());
                group.last_modified_time_millis = now;
            }
        }

The fallback that then reports the wrong generation, `state.rs:909-923`:

    pub fn effective_in(&self, billing_period: &str) -> Option<BillingGroupVersion> {
        ...
        versions
            .iter()
            .rev()
            .find(|version| {
                billing_period_key(&version.start_billing_period())
                    .is_some_and(|start| start <= wanted)
            })
            .or_else(|| versions.first())
            .cloned()

Test coverage: `tests/all/billing_group_billing_periods.rs:506` (`each_period_reports_the_generation_that_was_in_force_in_it`) exercises the generation history, but it builds multi-generation state with the direct-state helpers `created_in` / `generation_started_in` (`tests/all/billing_group_billing_periods.rs:191-216`) and never routes a write through `AssociateAccounts`, so nothing pins this path. `grep -rn 'put_billing_group' src/` matches only `state.rs:2055` and `update_billing_group.rs:363`.

## Steps to Reproduce

1. Create a billing group; in the store, treat the create as having happened in billing period M1 (the group's only generation starts at M1).
2. In a later billing period M2, call UpdateBillingGroup with a new Name ("B"). This goes through put_billing_group, so the M1 generation is pushed to superseded_versions and the current generation starts at M2.
3. Call ListBillingGroups with BillingPeriod = M2 and observe Name = "B" (correct).
4. In a still later billing period M3, call AssociateAccounts on the same group with one new account id.
5. Call ListBillingGroups with BillingPeriod = M2 again: the group now reports the M1 generation's Name ("A"), because associate_accounts.rs:262 moved last_modified_time_millis into M3 without pushing the M2 generation into superseded_versions.
