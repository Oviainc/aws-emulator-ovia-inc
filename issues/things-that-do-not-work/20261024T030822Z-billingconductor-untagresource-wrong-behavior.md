Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# UntagResource answers 404 for a live custom line item ARN, so its tags can never be removed

_Filed: 2026-10-24T03:08:22Z_
_Operation: UntagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UntagResource` resolves `ResourceArn` through `state.resource_tags_mut`, which returns `None` for every custom line item ARN. A `DELETE /tags/<customlineitem arn>?tagKeys=...` on an existing custom line item therefore answers HTTP 404 `NotFoundException` and removes nothing.

Expected: removing named tag keys from an existing custom line item succeeds (204) and the item's remaining tags are unchanged.

Actual: 404 `NotFoundException` for every custom line item ARN, so tags stored at `CreateCustomLineItem` time are permanently un-removable.

## Evidence

Spec: `UntagResource` input `ResourceArn` pattern includes the `customlineitem` form and the input declares `@references` to `com.amazonaws.billingconductor#CustomLineItem` (smithy_query operation=UntagResource).

Handler: `src/ops/untag_resource.rs` -> `state.resource_tags_mut(&input.resource_arn)` else `crate::state::tagging_not_found()`; `src/state.rs` `resource_tags_mut()` arm `TaggableResourceKind::CustomLineItem => None`, guarded by a stale comment about `CreateCustomLineItem` being a scaffold (it is implemented and stores tags on `CustomLineItemRecord`).

Tests: `tests/all/tagging_surface.rs` untags only pricing plans; its only customlineitem case is a never-created ARN asserted to be 404.

## Steps to Reproduce

1. Create a pricing plan, a billing group, then POST /create-custom-line-item with {"Tags":{"env":"prod","owner":"ops"}} and note the Arn
2. DELETE /tags/<url-encoded custom line item ARN>?tagKeys=env
3. Observe HTTP 404 NotFoundException instead of a 204 success with only the `owner` tag left
