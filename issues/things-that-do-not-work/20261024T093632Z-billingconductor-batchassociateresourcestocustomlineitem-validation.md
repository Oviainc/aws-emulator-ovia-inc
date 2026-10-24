Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a61b7e3a8f074f2acfd74e18fad3041c10471e17

# Resources from outside the target percentage custom line item's own billing group are associated instead of being refused

_Filed: 2026-10-24T09:36:32Z_
_Operation: BatchAssociateResourcesToCustomLineItem | Class: validation_

> _Note: this issue was identified against the code base at commit `a61b7e3a8f074f2acfd74e18fad3041c10471e17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler resolves each `ResourceArns` entry against the whole store and accepts it as long as (a) some stored billing group or custom line item carries that ARN and (b) it is not the target item itself. It never checks that the resource belongs to the SAME billing group as the target percentage custom line item.

Expected (AWS): a percentage custom line item's charge is "multiplied against the combined value of its associated resources" INSIDE the billing group the item applies to. The only PARENT a percentage item can have is its own `BillingGroupArn`, and a CHILD must be a custom line item of that same billing group. An ARN naming a different billing group, or a custom line item that lives in a different billing group, is an illegal association and is reported per-resource in `FailedAssociatedResources` (or refused for the request) with a `ValidationException`-class reason — `ValidationExceptionReason` enumerates exactly these cases: `ILLEGAL_CHILD_ASSOCIATE_RESOURCE`, `MISMATCHED_BILLINGGROUP_ARN`, `ILLEGAL_RESOURCE_ARNS`.

Actual: create billing groups A and B, a percentage custom line item in A, and a flat custom line item in B. Associating B's item (or billing group B itself) to A's percentage item answers 200 with the resource in `SuccessfullyAssociatedResources`, persists the association, bumps `AssociationSize`, and a later `ListResourcesAssociatedToCustomLineItem` reports it with `Relationship` CHILD (or PARENT for group B) — a cross-billing-group percentage basis the real service cannot compute.

## Evidence

Spec: `ValidationExceptionReason` (smithy_query query="shape" shape="ValidationExceptionReason") enumerates `ILLEGAL_CHILD_ASSOCIATE_RESOURCE`, `MISMATCHED_BILLINGGROUP_ARN` and `ILLEGAL_RESOURCE_ARNS`; `CustomLineItemPercentageChargeDetails.PercentageValue` is documented as "multiplied against the combined value of its associated resources", and `CustomLineItemRelationship` has exactly PARENT/CHILD, the parent being the item's own billing group (`CustomLineItemRecord.billing_group_arn` is set once, at create).

Handler: `src/ops/batch_associate_resources_to_custom_line_item.rs` lines ~277-298 — the only per-resource refusals are `INVALID_ARN` (resource resolves nowhere, via `crate::state::resolve_association_element`) and `ILLEGAL_CUSTOMLINEITEM` (self-association). `crate::state::resolve_association_element` (src/state.rs ~848-861) resolves against ALL stored custom line items and billing groups with no billing-group scoping, and `crate::state::association_relationship` derives PARENT/CHILD purely from the ARN's resource-type segment.

Tests: `tests/all/percentage_custom_line_item_resource_associations.rs` builds every target and resource inside ONE billing group; no test associates a resource from another billing group, so nothing pins the current permissive behaviour. The open issue 20261024T030822Z-billingconductor-batchassociateresourcestocustomlineitem-wrong-behavior.md is about the five-resource cap, a different class and a different defect.

## Steps to Reproduce

1. Make the emulated account the payer so the control plane is reachable (the crate's test seam sets `state.payer_account_id`)
2. POST /create-pricing-plan {"Name":"plan"} and note the plan ARN
3. POST /create-billing-group twice (groups A and B) with that plan, distinct names and distinct LinkedAccountIds
4. POST /create-custom-line-item in group A with ChargeDetails {"Type":"FEE","Percentage":{"PercentageValue":10}} — note the percentage item ARN
5. POST /create-custom-line-item in group B with ChargeDetails {"Type":"FEE","Flat":{"ChargeValue":5}} — note that item's ARN
6. PUT /batch-associate-resources-to-custom-line-item {"TargetArn":<A's percentage item>,"ResourceArns":[<B's flat item>]}
7. Observe 200 with the resource in SuccessfullyAssociatedResources instead of a FailedAssociatedResources entry / ValidationException for an illegal cross-billing-group association
8. POST /list-resources-associated-to-custom-line-item {"Arn":<A's percentage item>} — the foreign resource is reported as an association
