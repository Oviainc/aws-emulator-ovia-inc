Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: ca76825873aa90df46ea4feae577fc746e9d0a2a

# The generation-supersede rule and the effective-generation projection are each written out three times in state.rs, once per versioned resource

_Filed: 2026-10-29T05:33:56Z_
_Operation: AssociatePricingRules | Class: maintainability_

> _Note: this issue was identified against the code base at commit `ca76825873aa90df46ea4feae577fc746e9d0a2a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state.rs` implements the crate's billing-period versioning rule three times, once per versioned resource (pricing plan, pricing rule, billing group), with bodies that are identical apart from the record type, the map field and the `MAX_*_VERSIONS` constant:

1. The WRITE rule — "a write landing in a period AFTER the one the current generation started in closes that generation ... while a write landing in the generation's own period REPLACES it" — is `put_pricing_rule` (`state.rs:2988-3004`), `put_pricing_plan` (`state.rs:3021-3037`) and `put_billing_group` (`state.rs:3054-3070`). All three read the stored record, `filter` on `version_start_billing_period() != updated.version_start_billing_period()`, push `current_version()` onto `superseded_versions`, drop `remove(0)` past their own cap, and `insert`. The doc comments themselves say the three are one rule (`put_pricing_plan`: "The same rule [`Self::put_pricing_rule`] applies"; `put_billing_group`: the same sentence), so the duplication is already acknowledged in prose rather than removed in code.

2. The READ rule — `exists_in` plus `effective_in` ("The generation in force is the LATEST one whose own start period is at or before the period asked about", falling back to the oldest kept generation) — is written three times too: `PricingPlanRecord` (`state.rs:946-980`), `PricingRuleRecord` (`state.rs:1160-1194`) and `BillingGroupRecord` (`state.rs:1347-1381`). The three `versions()` / `current_version()` / `version_start_billing_period()` helpers around them are likewise triplicated.

Because there are three copies, a correction to one of them does not reach the other two: the crate's own history already shows this class of drift being fixed one resource at a time (the fixed issues `billingconductor-associateaccounts-wrong-behavior` and `billingconductor-disassociateaccounts-wrong-behavior` routed two billing-group writers through `put_billing_group` after the pricing-plan writers had already been routed through `put_pricing_plan`). The fix is to express the rule ONCE — a small trait over "a record that carries `superseded_versions`, projects a `current_version()` and derives `version_start_billing_period()` from `last_modified_time_millis`", with one generic `put_versioned` / `effective_in` implementation and the per-resource cap as an associated constant — and have the three resources use it. Every response and every existing test stays unchanged: the computation is byte-for-byte the same, only written once.

## Evidence

Copy 1 of the write rule — `src/state.rs:2988-3004`:
```
    pub fn put_pricing_rule(&mut self, mut updated: PricingRuleRecord) {
        let arn = updated.arn.clone();
        let superseded = self
            .pricing_rules
            .get(&arn)
            .filter(|previous| {
                previous.version_start_billing_period() != updated.version_start_billing_period()
            })
            .map(PricingRuleRecord::current_version);
        if let Some(superseded) = superseded {
            updated.superseded_versions.push(superseded);
            while updated.superseded_versions.len() > MAX_PRICING_RULE_VERSIONS {
                updated.superseded_versions.remove(0);
            }
```

Copy 2 — `src/state.rs:3021-3030`, under a doc that names copy 1 as the same rule (`src/state.rs:3009`: "The same rule [`Self::put_pricing_rule`] applies to a pricing rule"):
```
    pub fn put_pricing_plan(&mut self, mut updated: PricingPlanRecord) {
        let arn = updated.arn.clone();
        let superseded = self
            .pricing_plans
            .get(&arn)
            .filter(|previous| {
                previous.version_start_billing_period() != updated.version_start_billing_period()
            })
            .map(PricingPlanRecord::current_version);
        if let Some(superseded) = superseded {
```

Copy 3 — `src/state.rs:3054-3070`, under the same sentence again (`src/state.rs:3042`: "The same rule [`Self::put_pricing_rule`] applies to a pricing rule"):
```
    pub fn put_billing_group(&mut self, mut updated: BillingGroupRecord) {
        let arn = updated.arn.clone();
        let superseded = self
            .billing_groups
            .get(&arn)
            .filter(|previous| {
                previous.version_start_billing_period() != updated.version_start_billing_period()
            })
            .map(BillingGroupRecord::current_version);
        if let Some(superseded) = superseded {
            updated.superseded_versions.push(superseded);
            while updated.superseded_versions.len() > MAX_BILLING_GROUP_VERSIONS {
                updated.superseded_versions.remove(0);
            }
        }
        self.billing_groups.insert(arn, updated);
    }
```

The read rule, copy 1 — `src/state.rs:946-980`:
```
    pub fn exists_in(&self, billing_period: &str) -> bool {
        match (
            billing_period_key(billing_period),
            billing_period_key(&self.creation_billing_period()),
        ) {
            (Some(wanted), Some(created)) => wanted >= created,
            _ => false,
        }
    }
...
    pub fn effective_in(&self, billing_period: &str) -> Option<PricingPlanVersion> {
        if !self.exists_in(billing_period) {
            return None;
        }
        let wanted = billing_period_key(billing_period)?;
        let versions = self.versions();
        versions
            .iter()
            .rev()
            .find(|version| {
                billing_period_key(&version.start_billing_period())
                    .is_some_and(|start| start <= wanted)
            })
            .or_else(|| versions.first())
            .cloned()
    }
```
and copy 2, byte-identical modulo the version type — `src/state.rs:1160-1194`:
```
    pub fn exists_in(&self, billing_period: &str) -> bool {
        match (
            billing_period_key(billing_period),
            billing_period_key(&self.creation_billing_period()),
        ) {
            (Some(wanted), Some(created)) => wanted >= created,
            _ => false,
        }
    }
...
    pub fn effective_in(&self, billing_period: &str) -> Option<PricingRuleVersion> {
        if !self.exists_in(billing_period) {
            return None;
        }
        let wanted = billing_period_key(billing_period)?;
        let versions = self.versions();
        versions
            .iter()
            .rev()
            .find(|version| {
                billing_period_key(&version.start_billing_period())
                    .is_some_and(|start| start <= wanted)
            })
            .or_else(|| versions.first())
            .cloned()
    }
```
copy 3 is `BillingGroupRecord::exists_in` / `effective_in` at `src/state.rs:1347-1381`, with the same two bodies and `Option<BillingGroupVersion>`.

The three `versions()` bodies are also identical — `src/state.rs:934-938`, `src/state.rs:1148-1152`, `src/state.rs:1335-1339`:
```
        let mut versions = self.superseded_versions.clone();
        versions.push(self.current_version());
        versions
```

Call sites that must keep answering exactly the same: `src/ops/associate_pricing_rules.rs:297` and `src/ops/disassociate_pricing_rules.rs:272` (`state.put_pricing_plan(updated)`), `src/ops/update_pricing_plan.rs:262`, `src/ops/update_pricing_rule.rs:254` (`state.put_pricing_rule(updated)`), `src/ops/update_billing_group.rs:383`, `src/ops/associate_accounts.rs:284` and `src/ops/disassociate_accounts.rs:253` (`state.put_billing_group(...)`), plus the readers `src/ops/list_pricing_plans.rs:226,240`, `src/ops/list_pricing_rules.rs:234,249` and `src/ops/list_billing_groups.rs:308`.

Existing test coverage that pins the behaviour and must stay green unchanged: `tests/all/pricing_plan_billing_periods.rs`, `tests/all/pricing_rule_billing_periods.rs`, `tests/all/billing_group_billing_periods.rs`, `tests/all/membership_writes_keep_superseded_generations.rs`, `tests/all/pricing_plan_update.rs`, `tests/all/pricing_rule_update.rs`, `tests/all/update_billing_group.rs`, `tests/all/pricing_rule_association_period_subsets.rs` and `tests/all/state_roundtrip.rs`.

## Steps to Reproduce

1. grep -n 'pub fn put_pricing_rule\|pub fn put_pricing_plan\|pub fn put_billing_group' services/service-com-amazonaws-billingconductor-awsbillingconductor/src/state.rs — three definitions at :2988, :3021 and :3054.
2. Read the three bodies side by side: each differs only in the record type, the map field (`pricing_rules` / `pricing_plans` / `billing_groups`) and the `MAX_*_VERSIONS` constant; the supersede decision, the push, the oldest-first drop and the insert are the same lines three times.
3. grep -n 'pub fn effective_in\|pub fn exists_in\|pub fn versions' on the same file — three `exists_in` (:946, :1160, :1347), three `effective_in` (:965, :1179, :1366) and three `versions` (:934, :1148, :1335), each triple with identical bodies modulo the version type.
4. Note the doc comments at src/state.rs:3009 and src/state.rs:3042 both state "The same rule [`Self::put_pricing_rule`] applies" — the rule is already documented as one rule while being implemented three times.
5. Introduce one generic implementation (a trait carrying `superseded_versions`, `current_version()`, `version_start_billing_period()` and the per-resource version cap) and have the three resources delegate to it; run the full suite — tests/all/pricing_plan_billing_periods.rs, tests/all/pricing_rule_billing_periods.rs, tests/all/billing_group_billing_periods.rs, tests/all/membership_writes_keep_superseded_generations.rs and tests/all/state_roundtrip.rs still pass with no response change.
