Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2f84fd4c7d638614ea26115568d3a3113f96aac5

# An UpdatePricingPlan that changed nothing still moves LastModifiedTime and freezes a generation, unlike the two sibling updaters that state the opposite rule

_Filed: 2026-10-29T06:38:16Z_
_Operation: UpdatePricingPlan | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2f84fd4c7d638614ea26115568d3a3113f96aac5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdatePricingPlanInput` makes every member except `Arn` optional (`Name`, `Description`), so a request carrying only `Arn` — or one restating the `Name`/`Description` the plan already holds — reaches the handler and is served 200. On that path `update_pricing_plan.rs` advances `last_modified_time_millis` to `now_millis()` UNCONDITIONALLY, before it has compared the merged record against the stored one, and then writes through `state.put_pricing_plan(updated)` unconditionally too.

Two consequences for a call that modified no member of the plan:

1. `LastModifiedTime` moves. The model documents that member as "The most recent time when the pricing plan was modified", and the same value is what `ListPricingPlans` reports for the plan in the billing period the call was made in (it reads the generation in force, whose instant IS `last_modified_time_millis`). So a caller sees the plan as modified although nothing about it differs.

2. The plan's VERSION HISTORY gains an entry. `put_pricing_plan` decides whether to supersede purely by comparing `version_start_billing_period()` of the stored record with the incoming one, and `PricingPlanRecord::version_start_billing_period()` is derived from `last_modified_time_millis` alone. So a no-member update made in a billing period LATER than the plan's previous write freezes the current generation as a superseded one and opens a new generation whose every member is identical to it. Because the kept history is capped (`MAX_PRICING_PLAN_VERSIONS = 100`, oldest dropped first), a sequence of calls that changed nothing can evict generations that really did carry distinct state.

This crate implements the OPPOSITE rule for both sibling resources whose writes go through the structurally identical `put_pricing_rule` / `put_billing_group`: `UpdatePricingRule` computes `changed = !updated.holds_same_members_as(&stored)` and puts BOTH the timestamp move and the `put_pricing_rule` call inside `if changed`, with a comment stating the rule and naming `UpdateBillingGroup` as the precedent; `UpdateBillingGroup` does the same with an inline `changed` comparison. `UpdatePricingPlan` is the third updater of the same family and is the only one that does not.

The fix is the branch the crate already has twice: compare the merged record with the stored one on the members a generation carries (`PricingPlanVersion` is exactly `name` + `description` + `last_modified_time_millis`) and advance `last_modified_time_millis` and call `put_pricing_plan` only when something differs. The refusal paths (AWS-managed-plan refusal, 404, name conflict) and the derived `Size` are unchanged.

## Evidence

Smithy contract — `smithy_query(query="shape",shape="UpdatePricingPlanOutput")`:
```
"LastModifiedTime": {"target": "Instant", "default": 0, "type": "long",
  "doc": "<p> The most recent time when the pricing plan was modified. </p>"}
```
and the input that makes a no-op reachable — `smithy_query(query="shape",shape="UpdatePricingPlanInput")`: only `Arn` is `"required": true`; `Name` and `Description` are optional.

Unconditional advance and unconditional generation-keeping write — `services/service-com-amazonaws-billingconductor-awsbillingconductor/src/ops/update_pricing_plan.rs:225-235` and `:257-262`:
```
        // A member the request carries replaces the stored one; a member it omits keeps
        // what state holds. Nothing else about the plan — its rule associations, its
        // tags, its creation time — is this operation's to change.
        let mut updated = stored.clone();
        if let Some(name) = input.name.clone() {
            updated.name = name;
        }
        if let Some(description) = input.description.clone() {
            updated.description = Some(description);
        }
        updated.last_modified_time_millis = crate::state::now_millis();
...
        // A read of a pricing plan is scoped to a billing period, so the write is stored
        // as the plan's CURRENT generation and the generation it replaces is kept when
        // this write lands in a later period than that one started in: a closed period
        // still reports the plan as it stood then
        state.put_pricing_plan(updated);
```

The generation boundary is keyed on that very instant — `src/state.rs:910-918`:
```
    /// The billing period this plan's CURRENT generation is in effect FROM.
    ///
    /// A write takes effect in the period it is made in, and `LastModifiedTime` is the
    /// instant of the write that left the plan as it stands, so the period that instant
    /// falls in is the one the current generation starts in.
    pub fn version_start_billing_period(&self) -> String {
        billing_period_at(self.last_modified_time_millis)
    }
```
and `src/state.rs:3037-3052`:
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
            updated.superseded_versions.push(superseded);
            while updated.superseded_versions.len() > MAX_PRICING_PLAN_VERSIONS {
                updated.superseded_versions.remove(0);
            }
        }
```

Sibling 1, which states the rule this handler breaks — `src/ops/update_pricing_rule.rs:243-266`:
```
        // `LastModifiedTime` is "the most recent time the pricing rule was modified", so
        // it moves only when this call actually modified the rule: a request that names no
        // member, or names the values the rule already holds, leaves it where it was. The
        // generation in force starts in the billing period that instant falls in, so a
        // call that modified nothing writes no generation either — the same rule
        // `UpdateBillingGroup` applies to the sibling resource, decided by the state
        // method that names the members a generation carries
        // ([`crate::state::PricingRuleRecord::holds_same_members_as`]).
        let changed = !updated.holds_same_members_as(&stored);
        if changed {
            updated.last_modified_time_millis = crate::state::now_millis();
...
            state.put_pricing_rule(updated.clone());
        }
```

Sibling 2 — `src/ops/update_billing_group.rs:360-384`:
```
        // `LastModifiedTime` is "the most recent time when the billing group was
        // modified", so it moves only when this call actually modified the group: a
        // request that names no member, or names the values the group already holds,
        // leaves it where it was.
        let changed = updated.name != stored.name
            || updated.description != stored.description
...
        if changed {
            updated.last_modified_time_millis = crate::state::now_millis();
...
            state.put_billing_group(updated.clone());
        }
```

Test coverage, disclosed for the fixer: `tests/all/pricing_plan_update.rs:567-602` (`last_modified_time_moves_to_now_on_every_update`) currently pins the behaviour reported here, including the row `("an update naming no member", json!({}))`, on the stated reason at `:573-574`:
```
    // (case, the patch the update carries) — a rename, a description change and an
    // update that names nothing all move the timestamp, because the call is the event.
```
That reason ("the call is the event") is what the quoted Smithy sentence ("the most recent time when the pricing plan was modified") and the two sibling handlers' own comments contradict, so this test's no-member row has to change with the behaviour. The already-FIXED sibling issue `20261028T233252Z-billingconductor-updatepricingrule-wrong-behavior` explicitly names this file as the remaining copy of the opposite rule ("the crate currently holds BOTH rules across its three updaters"); the pinned rules `tests/all/update_billing_group.rs:1120` (`last_modified_time_moves_only_when_the_group_changed`) and `tests/all/pricing_rule_unchanged_update.rs:276` / `:358` (`last_modified_time_moves_only_when_the_update_changed_the_rule`, `an_update_that_changed_nothing_opens_no_generation`) stay unchanged.

## Steps to Reproduce

1. Designate the emulated account as the organization's payer account so the control plane is reachable (as `tests/all/pricing_plan_update.rs` `Emulator::new` does).
2. POST /create-pricing-plan with {"Name":"BasePlan"}; keep the returned Arn.
3. POST /list-pricing-plans and note the plan's LastModifiedTime (call it T0).
4. Wait a millisecond, then PUT /update-pricing-plan with {"Arn":"<plan arn>"} only — no Name, no Description.
5. Observe the 200 response's LastModifiedTime > T0, and a following POST /list-pricing-plans reporting the moved value, although no member of the plan differs.
6. Compare the sibling writers: PUT /update-pricing-rule with only {"Arn":"<rule arn>"}, and POST /update-billing-group with only {"Arn":"<group arn>"}, both leave their resource's LastModifiedTime at its previous value and write no generation.
7. To see the redundant generation: make the no-member UpdatePricingPlan in a billing period later than the create's (advance the store's clock), then read the plan back with POST /list-pricing-plans naming the create's billing period — it is now answered from a generation frozen by a call that changed nothing rather than from the create's own.
