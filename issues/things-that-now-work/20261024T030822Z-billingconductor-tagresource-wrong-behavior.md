Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# TagResource cannot tag a custom line item: 404 NotFoundException for a live customlineitem ARN

_Filed: 2026-10-24T03:08:22Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` resolves its `ResourceArn` through `state.resource_tags_mut`, whose `TaggableResourceKind::CustomLineItem` arm always returns `None`. Every `POST /tags/<customlineitem arn>` therefore answers HTTP 404 `NotFoundException` and stores nothing, even for a custom line item that exists in state.

Expected: tagging an existing custom line item succeeds (204) and the pairs merge into that item's tag map, exactly as it does for a billing group / pricing plan / pricing rule.

Actual: 404 `NotFoundException` ("Resource was not found") for every custom line item ARN; the custom line item's tag map can never be modified after create.

## Evidence

Spec: `TagResource` input `ResourceArn` pattern admits `^(arn:aws(-cn)?:billingconductor::[0-9]{12}:customlineitem/)?[a-zA-Z0-9]{10}$` and the input declares `@references` to `com.amazonaws.billingconductor#CustomLineItem` (smithy_query operation=TagResource), so a custom line item is part of this service's tagging surface.

Handler: `src/ops/tag_resource.rs` -> `let Some(stored) = state.resource_tags_mut(&input.resource_arn) else { return Err(crate::state::tagging_not_found()) }`; `src/state.rs` `resource_tags_mut()` arm `TaggableResourceKind::CustomLineItem => None` (stale comment claims CreateCustomLineItem is still a scaffold, but it is implemented and persists `custom_line_items`).

Tests: `tests/all/tagging_surface.rs` exercises plans and groups only; its customlineitem row is an ARN that was never created, so no test covers tagging a live custom line item.

## Steps to Reproduce

1. Create a pricing plan, a billing group, then POST /create-custom-line-item to get a custom line item ARN
2. POST /tags/<url-encoded custom line item ARN> with body {"Tags":{"env":"prod"}}
3. Observe HTTP 404 NotFoundException instead of a 204 success
4. GET /tags/<same ARN> — also 404, confirming nothing was stored
