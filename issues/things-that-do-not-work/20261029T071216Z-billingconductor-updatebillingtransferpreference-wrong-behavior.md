Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 37c02434d582c8370b608107fbfd29504c12ec2b

# UpdateBillingTransferPreference moves LastModifiedTime on a call that modified nothing, unlike the two sibling updaters that state the opposite rule

_Filed: 2026-10-29T07:12:16Z_
_Operation: UpdateBillingTransferPreference | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `37c02434d582c8370b608107fbfd29504c12ec2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateBillingTransferPreference` records the requested preference and stamps `LastModifiedTime` with `crate::state::now_millis()` UNCONDITIONALLY — it never compares the requested `AutoBillingTransferBillingGroupCreation` with the one the store already holds for that transfer. A request that restates exactly the preference the transfer already carries (same `Enabled`, same pricing plan — including the same plan spelled the other way the `PricingPlanArn` pattern admits, since the handler normalises it at `:229-232` before storing) is therefore served 200 with a NEW `LastModifiedTime`, and a following `GetBillingTransferPreference` reports that moved instant.

The model documents that member as "The most recent time when the preference was modified." A call that left `enabled` and `pricing_plan_arn` exactly as they were modified nothing, so the instant a reader sees must not move.

This crate implements the OPPOSITE rule for both sibling resources, each quoting the identical model wording for its own resource and gating BOTH the timestamp move and the store write on an explicit `changed` comparison: `UpdateBillingGroup` (`changed = updated.name != stored.name || ...`) and `UpdatePricingRule` (`changed = !updated.holds_same_members_as(&stored)`). `UpdateBillingTransferPreference` is the remaining writer of the family that does not — and it is the only one of the four updaters whose request can be reduced to a pure restatement with NO optional member at all, because both of its payload members are `@required`, so "restate what is already stored" is the only no-op form it has.

The path is not pinned by any test: the no-op restatement case does not appear in `tests/all/billing_transfer_preference.rs`. Every row of `get_reports_the_preference_the_last_update_persisted` changes something (`:353-427` — the "enabled, naming the same plan by its bare id" row is the ONLY setting on its own fresh transfer, and the multi-setting row repoints the plan and then disables it), and the idempotency tests at `:797` / `:885` cover the `ClientToken` replay path, which already does not move the instant.

The fix is the branch the crate already has twice: read the stored record and advance `LastModifiedTime` (i.e. call `set_billing_transfer_preference` with a fresh instant) only when `enabled` or the normalised `pricing_plan_arn` differs from what is held; a restatement answers with the stored record unchanged. The refusal paths (404 for an unknown transfer, `MISSING_PRICING_PLAN_ARN`, `FIELD_VALIDATION_FAILED`, `MISSING_PRICINGPLAN`), the `ClientToken` replay and the response shape all stay as they are. The state-module comment that currently describes the opposite rule (`src/state.rs:2125-2126`) has to be re-worded with it.

## Evidence

Smithy contract — `smithy_query(query="shape",shape="UpdateBillingTransferPreferenceOutput")`:
```
"LastModifiedTime": {"target": "Instant", "default": 0, "required": true, "type": "long",
  "doc": "<p>The most recent time when the preference was modified.</p>"}
```
and both payload members are `@required`, so a restatement is the operation's no-op form — `smithy_query(query="shape",shape="UpdateBillingTransferPreferenceInput")`:
```
"ResponsibilityTransferArn": {... "required": true ...},
"AutoBillingTransferBillingGroupCreation": {"target": "AutoTransferBillingGroupCreationPreference", "required": true, ...}
```

The unconditional stamp — `services/service-com-amazonaws-billingconductor-awsbillingconductor/src/ops/update_billing_transfer_preference.rs:273-283`:
```
        // The preference REPLACES whatever the transfer held: the whole preference is one
        // required member, so there is no member to carry over. Nothing is created here —
        // "the preference applies only to billing groups that are created after you
        // enable it", and the accounts that transfer their bill to the bill source
        // account are Organizations state this service does not hold.
        let stored = state.set_billing_transfer_preference(
            &transfer_arn,
            requested.enabled,
            plan_arn,
            crate::state::now_millis(),
        );
```
There is no comparison with the stored record anywhere in `handle` — the only read of it is the `ClientToken` replay at `:213-220`.

The store writes whatever instant it is handed — `src/state.rs:2550-2562`:
```
    pub fn set_billing_transfer_preference(
        &mut self,
        arn: &str,
        enabled: bool,
        pricing_plan_arn: Option<String>,
        now: i64,
    ) -> BillingTransferPreferenceRecord {
        let record = BillingTransferPreferenceRecord {
            responsibility_transfer_arn: arn.to_string(),
            enabled,
            pricing_plan_arn,
            last_modified_time_millis: now,
        };
```
and the response is rendered straight off it — `src/ops/update_billing_transfer_preference.rs:311-322`:
```
fn render(
    stored: &crate::state::BillingTransferPreferenceRecord,
) -> UpdateBillingTransferPreferenceOutput {
    UpdateBillingTransferPreferenceOutput {
...
        last_modified_time: stored.last_modified_time_millis,
    }
}
```

Sibling 1, which states the rule this handler breaks — `src/ops/update_billing_group.rs:360-384`:
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

Sibling 2 — `src/ops/update_pricing_rule.rs:243-266`:
```
        // `LastModifiedTime` is "the most recent time the pricing rule was modified", so
        // it moves only when this call actually modified the rule: a request that names no
        // member, or names the values the rule already holds, leaves it where it was.
...
        let changed = !updated.holds_same_members_as(&stored);
        if changed {
            updated.last_modified_time_millis = crate::state::now_millis();
```

The crate's own comment on the other side, which the quoted Smithy sentence contradicts and which the fix must re-word — `src/state.rs:2123-2128`:
```
/// `AutoTransferBillingGroupCreationPreference` carries `Enabled` and — only while the
/// preference is enabled — the `PricingPlanArn` the automatically created billing groups
/// are to compute with. The record adds the instant the preference was last set, which
/// is what `LastModifiedTime` reports; a transfer with NO record is how "the preference
/// has never been set" is represented, so that case reports the documented default and
/// an empty `LastModifiedTime` rather than a stored zero.
```

Test coverage, disclosed for the fixer: no test exercises a restatement of the stored preference. `tests/all/billing_transfer_preference.rs:459-468` asserts the instant moves between two settings that DIFFER (within the row "enabled, then repointed at another plan, then disabled", `:381-405`); `:797` (`a_retried_client_token_performs_no_further_action`, asserting at `:851-853` that "a retry must not move LastModifiedTime") and `:885` (`a_client_token_replays_only_for_the_transfer_it_recorded`) cover only the `ClientToken` replay, which the handler already answers from the stored record without writing. The pinned rules `tests/all/update_billing_group.rs:1120` (`last_modified_time_moves_only_when_the_group_changed`) and `tests/all/pricing_rule_unchanged_update.rs:276` (`last_modified_time_moves_only_when_the_update_changed_the_rule`) stay unchanged and are the shape of the row this operation's suite needs.

## Steps to Reproduce

1. Designate the emulated account as the organization's payer account and record one billing transfer, as `tests/all/billing_transfer_preference.rs` `Emulator::new` / `designate_transfer` do (the control plane is payer-gated and a transfer exists only once recorded).
2. PUT /update-billing-transfer-preference with {"ResponsibilityTransferArn":"<transfer arn>","AutoBillingTransferBillingGroupCreation":{"Enabled":true,"PricingPlanArn":"<a plan that exists>"}} and no X-Amzn-Client-Token. Note the 200 response's LastModifiedTime (call it T0).
3. POST /get-billing-transfer-preference for the same transfer and confirm it reports T0.
4. Wait a millisecond, then PUT /update-billing-transfer-preference again with the IDENTICAL body (or with the same plan named by its bare 10-character id, which the handler normalises to the same ARN), again with no ClientToken.
5. Observe the 200 response's LastModifiedTime > T0, and a following POST /get-billing-transfer-preference reporting the moved value, although neither Enabled nor the stored pricing plan differs — contradicting "The most recent time when the preference was modified".
6. Compare the sibling writers: POST /update-billing-group with only {"Arn":"<group arn>"} and PUT /update-pricing-rule with only {"Arn":"<rule arn>"} both leave their resource's LastModifiedTime at its previous value.
