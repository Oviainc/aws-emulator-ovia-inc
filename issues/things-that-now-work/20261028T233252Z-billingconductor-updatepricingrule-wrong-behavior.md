Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 92bcdac01f88c8b4612f5537a2ed608090c91758

# An UpdatePricingRule that changes nothing still freezes a new generation and moves LastModifiedTime, unlike the structurally identical UpdateBillingGroup

_Filed: 2026-10-28T23:32:52Z_
_Operation: UpdatePricingRule | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `92bcdac01f88c8b4612f5537a2ed608090c91758`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdatePricingRuleInput` makes every member except `Arn` optional, so a request carrying nothing but `Arn` reaches the handler and is served 200. On that path `UpdatePricingRule` sets `last_modified_time_millis = now_millis()` unconditionally — before it has compared the merged record against the stored one — and then writes through `state.put_pricing_rule(updated)`.

That has two effects for a call that changed no member of the rule:

1. `LastModifiedTime` moves. It is the value `ListPricingRules` reports for the rule in the billing period the call was made in (`list_pricing_rules.rs` reads it off the generation in force), so a caller sees the rule as modified although nothing about it differs.

2. The rule's VERSION HISTORY gains an entry. `put_pricing_rule` decides whether to supersede by comparing `version_start_billing_period()` of the stored record with that of the incoming one, and `version_start_billing_period()` is derived from `last_modified_time_millis` alone. So a no-member update made in a billing period later than the rule's previous write freezes the current generation as a superseded one and opens a new generation whose every member is identical to it. A read scoped to an earlier billing period is answered from a generation that was created by a call which modified nothing.

The crate implements the opposite rule for the sibling resource whose write goes through the structurally identical `put_billing_group`: `UpdateBillingGroup` computes a `changed` flag, and both the timestamp move and the `put_billing_group` call sit inside `if changed`, with a comment stating the rule. `DeleteCustomLineItem` applies the same discipline through its `PeriodsRemoval::Changed` / `PeriodsRemoval::Unchanged` arms. So one crate answers two opposite things for a write that changed nothing, and only the pricing-rule path mints a generation for it.

The fix is the branch the crate already has in `update_billing_group.rs`: compare the merged record with the stored one and both advance `last_modified_time_millis` and call `put_pricing_rule` only when something actually differs. The refusal paths (`merge`'s rate reconciliation, the name conflict) are unchanged.

## Evidence

Unconditional advance, then the generation-keeping write — `src/ops/update_pricing_rule.rs:235-236` and `:250-254`:
```
        let mut updated = merge(&stored, &input)?;
        updated.last_modified_time_millis = crate::state::now_millis();
...
        // A write takes effect in the billing period it is made in, so the rule the
        // period BEFORE this one reports is the generation this write supersedes rather
        // than the one it leaves ([`crate::state::ComAmazonawsBillingconductorAwsbillingconductorState::put_pricing_rule`]).
        state.put_pricing_rule(updated);
```
`merge` (`src/ops/update_pricing_rule.rs:277-317`) replaces only the members the request carries (`if let Some(name) = input.name.clone() { ... }` etc.), so a body of `{"Arn": "<rule>"}` reaches `:236` having changed nothing.

The generation boundary is keyed on that very instant — `src/state.rs:1121-1129`:
```
    /// The billing period this rule's CURRENT generation is in effect FROM.
    ///
    /// A write takes effect in the period it is made in, and `LastModifiedTime` is the
    /// instant of the write that left the rule as it stands, so the period that instant
    /// falls in is the one the current generation starts in.
    pub fn version_start_billing_period(&self) -> String {
        billing_period_at(self.last_modified_time_millis)
    }
```
and `src/state.rs:2949-2963`:
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
```

The opposite rule, stated and implemented in this crate for the sibling resource — `src/ops/update_billing_group.rs:360-384`:
```
        // `LastModifiedTime` is "the most recent time when the billing group was
        // modified", so it moves only when this call actually modified the group: a
        // request that names no member, or names the values the group already holds,
        // leaves it where it was.
        let changed = updated.name != stored.name
            || updated.description != stored.description
            || updated.pricing_plan_arn != stored.pricing_plan_arn
            || updated.requested_status != stored.requested_status
            || updated.auto_associate != stored.auto_associate;
        if changed {
            updated.last_modified_time_millis = crate::state::now_millis();
...
            state.put_billing_group(updated.clone());
        }
```
and for the custom line item — `src/ops/delete_custom_line_item.rs:218-233`:
```
                    crate::state::PeriodsRemoval::Changed => {
                        // The record changed, so the instant a read reports as its most
                        // recent modification advances with it.
                        item.last_modified_time_millis = crate::state::now_millis();
...
                    // The request named only billing periods the item never applied in,
                    // so there is nothing of it to delete from them and no stored value
                    // changes.
                    crate::state::PeriodsRemoval::Unchanged => ("unchanged", false),
```

Test coverage: nothing pins the pricing rule's behaviour either way — `tests/all/pricing_rule_update.rs:442-448` asserts only that the reported instant is positive:
```
            let last_modified = reported
                .get("LastModifiedTime")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            assert!(
                last_modified > 0,
```
while the sibling rule IS pinned, by `tests/all/update_billing_group.rs:1120` `last_modified_time_moves_only_when_the_group_changed`. Note for the fixer: `tests/all/pricing_plan_update.rs:573-579` states the opposite for `UpdatePricingPlan` ("an update that names nothing all move the timestamp, because the call is the event"), so the crate currently holds BOTH rules across its three updaters — this issue reports the pricing-RULE path, which is the one that additionally writes a redundant generation and which no test pins.

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable.
2. POST /create-pricing-rule with {"Name":"R1","Scope":"GLOBAL","Type":"MARKUP","ModifierPercentage":10}; keep the returned Arn.
3. POST /list-pricing-rules and note the rule's LastModifiedTime (call it T0).
4. Wait a millisecond, then PUT /update-pricing-rule with {"Arn":"<rule arn>"} only — no Name, no Description, no Type, no ModifierPercentage, no Tiering.
5. Observe the 200 response's LastModifiedTime > T0, and a following POST /list-pricing-rules reporting the moved value, although no member of the rule differs.
6. Compare: POST /update-billing-group with only {"Arn":"<group arn>"} leaves the group's LastModifiedTime at its previous value and writes no generation.
7. To see the extra generation: advance the store's clock (or make the no-member update in a later billing period than the create), then read the rule back with POST /list-pricing-rules naming the create's billing period — it is now answered from a generation that was frozen by the call that changed nothing rather than from the create's own.
