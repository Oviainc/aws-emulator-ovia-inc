Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e42d60e9c80231c2e4470b4ab9cec2534ece4a72

# ListResourcesAssociatedToCustomLineItem applies no billing-period scoping when BillingPeriod is omitted, so ended associations are still reported

_Filed: 2026-10-24T18:16:34Z_
_Operation: ListResourcesAssociatedToCustomLineItem | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e42d60e9c80231c2e4470b4ab9cec2534ece4a72`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The operation reports the resource associations of a percentage custom line item for a billing period. Two period-related divergences:

1. When the request omits BillingPeriod the handler skips the period check entirely instead of defaulting to the CURRENT billing period (the convention this service documents for its period-scoped reads — ListCustomLineItems: "If you don't provide a billing period, the current billing period is used", and the sibling handler in this same crate implements it). So an association that a bounded BatchDisassociateResourcesFromCustomLineItem has already ENDED (that call sets the association's ExclusiveEndBillingPeriod to the range's start, it does not drop the record) keeps being reported by a plain ListResourcesAssociatedToCustomLineItem, together with an EndBillingPeriod in the past. Expected: the default read reports only the associations in effect in the current billing period, so a disassociated resource disappears from the default listing.

2. An association made with no BillingPeriodRange is stored with an OPEN start (inclusive_start = None), so a request naming any earlier billing period reports it too — the association is reported for periods before the custom line item, the resource or the association itself existed. Expected: an association cannot be in effect in a billing period that precedes it (the same defect already filed for ListCustomLineItems reporting items for periods preceding their creation).

Because AssociationSize elsewhere counts the same records, the two views agree with each other but not with the period a caller asked about.

## Evidence

Spec: ListResourcesAssociatedToCustomLineItemInput$BillingPeriod is "The billing period for which the resource associations will be listed" (smithy_query query="operation"); ListCustomLineItems/ListBillingGroups document the current-billing-period default for the same member, and ResourceAssociationRecord's EndBillingPeriod is reported per association.

Handler: services/service-com-amazonaws-billingconductor-awsbillingconductor/src/ops/list_resources_associated_to_custom_line_item.rs lines ~202-226 — `if let Some(billing_period) = &input.billing_period { if !association.covers(billing_period) { continue; } }`: no `unwrap_or_else(crate::state::current_billing_period)` as in src/ops/list_custom_line_items.rs line ~264. src/ops/batch_disassociate_resources_from_custom_line_item.rs `apply_disassociation` keeps the record and only sets `exclusive_end_billing_period` when the call named a range, and src/state.rs `ResourceAssociationRecord::covers` treats an absent start as the unbounded past.

Tests: tests/all/percentage_custom_line_item_resource_associations.rs and tests/all/batch_disassociate_resources_from_custom_line_item.rs exercise this listing, but no test asserts the omitted-BillingPeriod default or a period earlier than the association. No existing issue names this operation.

## Steps to Reproduce

1. Designate the emulated account as the payer account so the control plane is reachable.
2. Create a pricing plan, a billing group and a percentage custom line item in that group (POST /create-custom-line-item with ChargeDetails.Percentage).
3. PUT /batch-associate-resources-to-custom-line-item with {"TargetArn":"<item arn>","ResourceArns":["<billing group arn>"]} and no BillingPeriodRange.
4. PUT /batch-disassociate-resources-from-custom-line-item with the same TargetArn/ResourceArns and {"BillingPeriodRange":{"InclusiveStartBillingPeriod":"<current billing period>"}} — the association is ended as of the current period.
5. POST /list-resources-associated-to-custom-line-item with only {"Arn":"<item arn>"} (no BillingPeriod) and observe the disassociated resource still listed, with EndBillingPeriod set to the current period, instead of an empty AssociatedResources for the current period.
6. Separately, after a plain association with no BillingPeriodRange, POST /list-resources-associated-to-custom-line-item with {"Arn":"<item arn>","BillingPeriod":"2020-01"} and observe the association reported for a period long before it existed.
