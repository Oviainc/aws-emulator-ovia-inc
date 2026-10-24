Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 1f773e23b3e0bd2b9ce8ba90d57212de869e61a4

# TagResource accepts tag keys using the reserved aws: prefix instead of refusing them

_Filed: 2026-10-24T08:46:21Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `1f773e23b3e0bd2b9ce8ba90d57212de869e61a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `aws:` tag-key prefix is reserved for AWS use: AWS tagging restrictions state that you may not create, edit or delete a tag whose key starts with `aws:` through a user-facing tagging API, and services refuse such a request with their parameter-fault error (for Recycle Bin that is the modeled `ValidationException`, HTTP 400, with `Reason=INVALID_PARAMETER_VALUE`). The model's `TagKey` pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` admits a colon, so the framework's constraint walk cannot catch it — the handler has to.

Expected: `POST /tags/{ResourceArn}` with `Tags:[{"Key":"aws:cloudformation:stack-name","Value":"x"}]` is refused with HTTP 400 ValidationException and the rule's tag set is left untouched (nothing from the refused call is stored, including any well-formed tag that travelled beside the reserved one).

Actual: `src/ops/tag_resource.rs` checks only that the ARN resolves to a rule and that the resulting tag count stays within the tags-per-resource quota, then calls `rule.apply_tags(input.tags)`. The system tag is persisted and subsequently reported by ListTagsForResource and by CreateRule/GetRule-side tag reads as if the caller owned it — a tag the real service would never let a caller create. The same `remove_tags` path on UntagResource will also happily delete a key carrying the reserved prefix, which the real service refuses for the same reason.

## Evidence

Semantics: AWS General Reference, "Tag naming limits and requirements" — the `aws:` prefix is reserved for AWS use; you cannot create or edit tag keys or values with this prefix through the tagging APIs. This is enforced elsewhere in this repo as established behavior: `services/service-com-amazonaws-xray-awsxray/tests/all/resource_tags.rs` has the case "a key using the reserved aws: prefix" (`aws:cloudformation:stack-name`) asserting a 400 refusal that stores nothing, and the same suite covers "the reserved aws: prefix, which marks a system tag".

Model: TagResource declares `ValidationException (HTTP 400)` (OPERATIONS.md "## TagResource"); `com.amazonaws.rbin#Tag$Key` is `TagKey` with length 1..128 and pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, which permits `aws:` — so the pre-dispatch gate in `src/service.rs` (which enforces exactly the model's constraints and nothing more) passes it through.

Handler: `services/service-com-amazonaws-rbin-amazonrecyclebin/src/ops/tag_resource.rs` — the only guards are `store.rule_by_arn_mut(&input.resource_arn)` (404) and `rule.tag_count_after(&input.tags) > MAX_TAGS_PER_RULE` (402); then `rule.apply_tags(input.tags)` unconditionally upserts whatever keys arrived. No key inspection exists in `state.rs::upsert_tags` either.

Test coverage: none. `tests/all/resource_tags.rs` tag sets (`tag_set_cases`, `tag_resource_upserts_by_key_and_leaves_every_other_key_alone`, `tag_resource_is_refused_past_the_published_tags_per_resource_quota`) use only caller-owned keys; no test sends a key with the `aws:` prefix to TagResource or UntagResource.

(The CreateRule `Tags` path shares the root cause, but a CreateRule/validation issue is already open — 20261024T020921Z-rbin-createrule-validation — so it is not re-filed here.)

## Steps to Reproduce

1. POST /rules with {"ResourceType":"EBS_SNAPSHOT","RetentionPeriod":{"RetentionPeriodValue":7,"RetentionPeriodUnit":"DAYS"}} — 201; note RuleArn.
2. POST /tags/{RuleArn} with {"Tags":[{"Key":"aws:cloudformation:stack-name","Value":"my-stack"},{"Key":"Also","Value":"valid"}]} — expect HTTP 400 ValidationException (reserved prefix) with nothing stored; actual HTTP 201 and both tags persisted.
3. GET /tags/{RuleArn} — expect Tags to be empty (the refused call stored nothing); actual Tags contains the reserved key aws:cloudformation:stack-name alongside Also=valid.
4. DELETE /tags/{RuleArn}?tagKeys=aws:cloudformation:stack-name — expect HTTP 400 ValidationException (a system tag cannot be removed by a caller); actual HTTP 204 and the tag is deleted.
