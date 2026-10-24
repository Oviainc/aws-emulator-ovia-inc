Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 1f773e23b3e0bd2b9ce8ba90d57212de869e61a4

# TagResource accepts reserved aws:-prefixed tag keys

_Filed: 2026-10-24T08:44:29Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `1f773e23b3e0bd2b9ce8ba90d57212de869e61a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AWS reserves the `aws:` tag-key prefix for AWS-generated tags; tagging APIs refuse a user-supplied key beginning with `aws:` with a validation error and store nothing. The modelled `ResourceTagKey` pattern (`^[\S\s]*$`) cannot express that rule, so it must be enforced by the handler.

Expected: TagResource with resourceTags [{key: "aws:created-by", value: "x"}] is refused with the modelled ValidationException (400) and the resource's tag set is unchanged.

Actual: the handler merges whatever keys the request carries, so the reserved key is persisted and read back by ListTagsForResource as an ordinary tag.

## Evidence

Spec: `ResourceTag.key` is a free-form `ResourceTagKey` (smithy_query shape=ResourceTag), i.e. the model cannot encode the reserved prefix; the `aws:` prefix reservation is well-established AWS tagging behavior shared by every tag-on-create/TagResource API.
Handler: `src/ops/tag_resource.rs:190-219` merges `input.resource_tags` by key and only checks the per-resource count ceiling — no key inspection at all.
Tests: `tests/all/tag_resource.rs` and `tests/all/resource_tags.rs` exercise merge/overwrite and not-found paths; no test sends an `aws:`-prefixed key.

## Steps to Reproduce

1. CreateDashboard, note the dashboard ARN
2. TagResource with resourceArn = that ARN and resourceTags [{"key": "aws:cost-center", "value": "1"}]
3. Observe HTTP 200; expected HTTP 400 ValidationException about the reserved aws: prefix
4. ListTagsForResource → the reserved key is reported as a stored tag
