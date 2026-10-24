Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# Associations past five resources are failed with SERVICE_LIMIT_EXCEEDED, although one request may name 30 resource ARNs

_Filed: 2026-10-24T03:08:22Z_
_Operation: BatchAssociateResourcesToCustomLineItem | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler refuses any association that would take a custom line item past `crate::state::MAX_RESOURCES_PER_CUSTOM_LINE_ITEM = 5`, pushing the extra resources into `FailedAssociatedResources` with reason `SERVICE_LIMIT_EXCEEDED`.

Expected: a single `BatchAssociateResourcesToCustomLineItem` request naming up to the modeled 30 `ResourceArns` associates all of them to a percentage custom line item and reports them all in `SuccessfullyAssociatedResources`.

Actual: only the first five associations on an item ever succeed; a request naming six or more distinct, existing resources returns 200 with the sixth and later entries in `FailedAssociatedResources`, and a later `ListResourcesAssociatedToCustomLineItem` reports at most five associations. The cap is a crate-chosen bound derived from the CREATE shape's `AssociatedValues` `@length(max: 5)`, not from this operation's own contract.

## Evidence

Spec: `BatchAssociateResourcesToCustomLineItem.ResourceArns` is `@length(min: 1, max: 30)` (OPERATIONS.md, smithy_query operation=BatchAssociateResourcesToCustomLineItem), so the service accepts 30 resources in ONE call — a per-item ceiling of five makes 25 of them fail, and the limit accumulates across calls as well.

Handler: `src/ops/batch_associate_resources_to_custom_line_item.rs` lines ~339-350 (`if target.associated_resources.len() >= crate::state::MAX_RESOURCES_PER_CUSTOM_LINE_ITEM { failures.push(failed(requested, "SERVICE_LIMIT_EXCEEDED", ...)) }`); `src/state.rs` `pub const MAX_RESOURCES_PER_CUSTOM_LINE_ITEM: usize = 5` whose own doc comment admits it is "A CHOSEN bound, informed by the model rather than read off it" and that the documented AWS quota is not in the model.

Tests: `tests/all/percentage_custom_line_item_resource_associations.rs` exercises small association sets; nothing asserts a 30-ARN request succeeds, and nothing asserts the five-item cap as correct AWS behavior.

## Steps to Reproduce

1. Create a pricing plan and a billing group
2. Create six flat custom line items in that billing group (to use as association targets) plus one percentage custom line item
3. PUT /batch-associate-resources-to-custom-line-item {"TargetArn":<percentage item arn>,"ResourceArns":[<six existing custom line item ARNs>]}
4. Observe 200 with five entries in SuccessfullyAssociatedResources and the sixth in FailedAssociatedResources with reason SERVICE_LIMIT_EXCEEDED, instead of all six associated
5. POST /list-resources-associated-to-custom-line-item {"Arn":<percentage item arn>} — only five associations are reported
