Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 89bd99e3e826befdb8e4111b5c2027c562b7fa6c

# Seven state-changing handlers write no audit record, although the crate states that every write of this service emits one

_Filed: 2026-10-28T23:02:25Z_
_Operation: CreateCustomLineItem | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `89bd99e3e826befdb8e4111b5c2027c562b7fa6c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate gives every state-changing call an audit record — a `tracing::info!` naming the operation, the caller's access key id, the ARN it addressed and the outcome — and says so in the comments above those records: `AssociateAccounts` calls it "the shape every other write of this service emits", `CreateBillingGroup` writes it "as `UpdateBillingGroup` writes it for the same resource", `CreatePricingPlan` "as `CreatePricingRule` writes it for the sibling resource".

That claim is false for seven of the service's writes, which take no audit record at all and discard `ctx` with `let _ = ctx;` instead of reading the caller's access key id out of it:

* `CreateCustomLineItem` — mints a resource and leaves no record, where its own siblings `CreateBillingGroup`, `CreatePricingPlan` and `CreatePricingRule` all write one, and where `UpdateCustomLineItem` and `DeleteCustomLineItem` write one for the SAME resource;
* `DeleteBillingGroup` — deletes a group, its custom line items and its account memberships, where `CreateBillingGroup` and `UpdateBillingGroup` both record their writes;
* `DeletePricingRule` — where the sibling delete `DeletePricingPlan` records its own;
* `BatchAssociateResourcesToCustomLineItem` and `BatchDisassociateResourcesFromCustomLineItem` — where the analogous `AssociatePricingRules` / `DisassociatePricingRules` and `AssociateAccounts` / `DisassociateAccounts` pairs all record theirs;
* `TagResource` and `UntagResource` — the only writes that mutate a resource's stored tags.

So the attribution discipline the crate states for "a state-changing call" holds for 14 of its 21 writes and silently not for the other 7: the operations that destroy a billing group with everything under it, that mint a custom line item, and that edit tags are exactly the ones whose writes cannot be attributed to a caller. The defect is the missing record in those handlers, not the wording of the comments that promise it.

## Evidence

The rule, stated in this crate — `src/ops/associate_accounts.rs:292-304`:
```
        // The audit record for a state-changing call: who asked, what changed and that
        // it succeeded — the shape every other write of this service emits
        // (`UpdateBillingGroup`). Never a `@sensitive` member of the group, and not the
        // account ids themselves, only how many came in, the access key id the caller
        // presented and the ARN it addressed.
        ::tracing::info!(
            operation = Self::NAME,
            akid = %::engine::extract_akid(&ctx.parts().headers),
            billing_group_arn = %group_arn,
            associated_accounts = added,
            outcome = if added == 0 { "unchanged" } else { "associated" },
            "accounts associated with a billing group"
        );
```
and restated by the sibling creates — `src/ops/create_billing_group.rs:451-461` ("The audit record for a state-changing call, as `UpdateBillingGroup` writes it for the same resource: who asked, what was minted and that it succeeded.") and `src/ops/create_pricing_plan.rs:288-300` ("as `CreatePricingRule` writes it for the sibling resource").

The writes that break it — each discards `ctx` and emits nothing (`grep -n 'tracing::info!' src/ops/*.rs` lists only 14 of the 21 writes):
* `src/ops/create_custom_line_item.rs:208`: `        let _ = ctx;` — the handler mints the item (`:417-418` `state.custom_line_items.insert(arn.clone(), record);`) and returns at `:420` with no record, while `src/ops/delete_custom_line_item.rs:259-269` writes one for the same resource ("The audit record for a state-changing call: who asked, what it addressed and what it did.").
* `src/ops/delete_billing_group.rs:158`: `        let _ = ctx;` — removal at `:207` `let removed = state.remove_billing_group(&group_arn);`, no record.
* `src/ops/delete_pricing_rule.rs:148`: `        let _ = ctx;` — removal at `:209` `state.remove_pricing_rule(&rule_arn);`, while `src/ops/delete_pricing_plan.rs:253-262` records its own delete.
* `src/ops/batch_associate_resources_to_custom_line_item.rs:210` and `src/ops/batch_disassociate_resources_from_custom_line_item.rs:244`: `        let _ = ctx;` — both mutate `target.associated_resources` / `target.last_modified_time_millis`.
* `src/ops/tag_resource.rs:168` and `src/ops/untag_resource.rs:160`: `        let _ = ctx;` — both write through `state.resource_tags_mut(...)` (`tag_resource.rs:227-229`, `untag_resource.rs:190-192`).

Test coverage: none either way — `grep -rn 'tracing\|audit\|akid' tests/` matches nothing, so no suite pins the presence or the absence of a record and adding the missing ones changes no response and no existing test.

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable.
2. Run the emulator with the tracing subscriber capturing INFO events.
3. POST /create-billing-group, then POST /create-custom-line-item against that group with a Flat ChargeDetails.
4. Observe one INFO event for the billing-group create (operation=CreateBillingGroup, akid, billing_group_arn, outcome="created") and NO event at all for the custom-line-item create, although it minted and persisted a resource.
5. POST /update-custom-line-item and POST /delete-custom-line-item against the same item: both emit their audit record, so the gap is the create alone for that resource.
6. Repeat with POST /delete-billing-group, POST /delete-pricing-rule, PUT /batch-associate-resources-to-custom-line-item, PUT /batch-disassociate-resources-from-custom-line-item, POST /tags/{ResourceArn} and DELETE /tags/{ResourceArn}: every one of them changes stored state and emits no event, while DeletePricingPlan, AssociatePricingRules and DisassociatePricingRules emit theirs.
