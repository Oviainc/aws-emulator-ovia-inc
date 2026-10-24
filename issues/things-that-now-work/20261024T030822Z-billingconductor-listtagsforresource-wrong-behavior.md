Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# ListTagsForResource answers 404 for a live custom line item ARN, so tags written by CreateCustomLineItem are unreadable

_Filed: 2026-10-24T03:08:22Z_
_Operation: ListTagsForResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The tagging surface resolves `ResourceArn` for billing groups, pricing plans and pricing rules, but NEVER for custom line items: `ComAmazonawsBillingconductorAwsbillingconductorState::resource_tags` returns `None` for `TaggableResourceKind::CustomLineItem` unconditionally, so `ListTagsForResource` falls into `crate::state::tagging_not_found()` and answers HTTP 404 `NotFoundException` ("Resource was not found").

Expected: after `CreateCustomLineItem` with `Tags`, `GET /tags/arn:aws:billingconductor::<account>:customlineitem/<id>` reports exactly those tags (a custom line item is a taggable resource of this service, and the create persists its `Tags` map).

Actual: 404 `NotFoundException` for every custom line item ARN, including one that was just created and whose record (with its tags) is in `state.custom_line_items`. The tags the create stored are therefore write-only and unobservable.

## Evidence

Spec: `ListTagsForResource` input `ResourceArn` pattern includes `^(arn:aws(-cn)?:billingconductor::[0-9]{12}:customlineitem/)?[a-zA-Z0-9]{10}$`, and the input carries `@references` to `com.amazonaws.billingconductor#CustomLineItem` alongside BillingGroup/PricingPlan/PricingRule (smithy_query operation=ListTagsForResource). `CreateCustomLineItem` models a `Tags` map and the handler persists it (`src/ops/create_custom_line_item.rs`: `tags: input.tags.unwrap_or_default().into_iter().collect()` on `CustomLineItemRecord`).

Handler: `src/ops/list_tags_for_resource.rs` -> `state.resource_tags(&input.resource_arn)`; `src/state.rs` `resource_tags()` has the arm `TaggableResourceKind::CustomLineItem => None` with the now-stale comment "Nothing in this slice persists a custom line item (`CreateCustomLineItem` is still a scaffold)" — CreateCustomLineItem is implemented.

Tests: `tests/all/tagging_surface.rs::every_tagging_operation_answers_404_not_found_for_an_unknown_resource` only asserts 404 for a customlineitem ARN that was NEVER created ("a custom line item ARN (nothing persists one yet)"); no test creates a custom line item and reads its tags, so nothing asserts the current behavior for a live item.

## Steps to Reproduce

1. POST /create-pricing-plan {"Name":"Plan1"} and POST /create-billing-group with that plan to get a billing group ARN
2. POST /create-custom-line-item {"Name":"Item1","Description":"d","BillingGroupArn":<group arn>,"ChargeDetails":{"Type":"FEE","Flat":{"ChargeValue":10}},"Tags":{"env":"prod"}} and note the returned Arn
3. GET /tags/<url-encoded custom line item ARN>
4. Observe HTTP 404 with error code NotFoundException instead of 200/204 with {"Tags":{"env":"prod"}}
