Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a61b7e3a8f074f2acfd74e18fad3041c10471e17

# CreateCustomLineItem accepts AssociatedValues naming resources in another billing group and an AccountId that is not in the target billing group

_Filed: 2026-10-24T09:36:32Z_
_Operation: CreateCustomLineItem | Class: validation_

> _Note: this issue was identified against the code base at commit `a61b7e3a8f074f2acfd74e18fad3041c10471e17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two input members are stored without being checked against the billing group the item applies to:

1. `ChargeDetails.Percentage.AssociatedValues` — each value only has to resolve to SOME stored billing group or custom line item. A value naming a billing group other than `BillingGroupArn`, or a custom line item belonging to another billing group, is accepted and persisted as an association (PARENT/CHILD derived from the ARN alone). Expected: the associated resources of a percentage custom line item are the item's own billing group or custom line items of that same billing group; anything else is refused with the modelled `ValidationException` (`ValidationExceptionReason` enumerates `ILLEGAL_CHILD_ASSOCIATE_RESOURCE`, `MISMATCHED_BILLINGGROUP_ARN`, `ILLEGAL_RESOURCE_ARNS`).

2. `AccountId` — "The Amazon Web Services account in which this custom line item will be applied to" is stored verbatim (any 12-digit id passes the modelled pattern), even when that account is not one of the target billing group's `LinkedAccountIds`. Expected: an account outside the billing group cannot carry the item's charge and the request is refused (`ILLEGAL_ACCOUNT_ID` / `ILLEGAL_ACCOUNTS`). Actual: 200, and `ListCustomLineItems` with `Filters.AccountIds` later reports the item under an account the group does not bill.

## Evidence

Spec: `ValidationExceptionReason` enumerates `ILLEGAL_CHILD_ASSOCIATE_RESOURCE`, `MISMATCHED_BILLINGGROUP_ARN`, `ILLEGAL_RESOURCE_ARNS`, `ILLEGAL_ACCOUNT_ID` and `ILLEGAL_ACCOUNTS` (smithy_query query="shape" shape="ValidationExceptionReason"); `CreateCustomLineItemInput.BillingGroupArn` is the group "where the custom line item applies to" and `AccountId` the account "in which this custom line item will be applied to"; `CustomLineItemPercentageChargeDetails.PercentageValue` is "multiplied against the combined value of its associated resources".

Handler: `src/ops/create_custom_line_item.rs` lines ~258-298 resolve each `AssociatedValues` entry with `crate::state::resolve_association_element` (existence + duplicate only — `src/state.rs` ~848-861 searches every stored custom line item and billing group, unscoped), and line ~328 stores `account_id: input.account_id` with no membership check against `state.billing_groups[billing_group_arn].linked_account_ids`.

Tests: `tests/all/percentage_custom_line_item_resource_associations.rs::creating_a_custom_line_item_refuses_what_it_cannot_persist` covers only non-existent and duplicated values, all inside one billing group; no test covers a foreign billing group's resource or a non-member AccountId, so nothing pins today's behaviour.

## Steps to Reproduce

1. Make the emulated account the payer so the control plane is reachable
2. POST /create-pricing-plan, then POST /create-billing-group twice (A with LinkedAccountIds ["111111111111"], B with ["222222222222"])
3. POST /create-custom-line-item in group B with a flat charge — note its ARN
4. POST /create-custom-line-item {"BillingGroupArn":<A>,"Name":"pct","Description":"d","ChargeDetails":{"Type":"FEE","Percentage":{"PercentageValue":10,"AssociatedValues":[<B's item ARN>]}}}
5. Observe 200 and a persisted cross-group association (POST /list-resources-associated-to-custom-line-item on the new item reports it) instead of a ValidationException
6. POST /create-custom-line-item {"BillingGroupArn":<A>,"Name":"flat","Description":"d","AccountId":"999999999999","ChargeDetails":{"Type":"FEE","Flat":{"ChargeValue":1}}}
7. Observe 200 and POST /list-custom-line-items {"Filters":{"AccountIds":["999999999999"]}} returning the item, although that account is not a member of billing group A
