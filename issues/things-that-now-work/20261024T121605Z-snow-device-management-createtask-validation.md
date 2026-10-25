Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 9e66ef5d78d13d37b68af2ed28461695d94c8b52

# CreateTask accepts tags that TagResource refuses (reserved aws: prefix, empty key, over-long key/value, >50 tags)

_Filed: 2026-10-24T12:16:05Z_
_Operation: CreateTask | Class: validation_

> _Note: this issue was identified against the code base at commit `9e66ef5d78d13d37b68af2ed28461695d94c8b52`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateTask takes the same TagMap as TagResource ("Optional metadata that you assign to a resource"). The AWS-wide tagging rules apply on create exactly as they do on TagResource: a tag key may not begin with the reserved `aws:` prefix, may not be empty, keys are limited to 128 and values to 256 Unicode characters, and a resource may carry at most 50 tags. A CreateTask carrying such a tag map is refused with ValidationException (HTTP 400) and no task is created.

The emulator applies none of these checks on the create path: the handler stores `input.tags.unwrap_or_default()` verbatim. So a task is minted (HTTP 200, with a taskId and taskArn) carrying e.g. `aws:created-by` or 200 tags, and DescribeTask / ListTasks / ListTagsForResource then report tags the service would never have accepted — while the very same map on POST /tags/{taskArn} is refused 400. The crate already has the rule implemented in `crate::state::tag_violation`; CreateTask simply never calls it.

## Evidence

Spec: CreateTask input member `tags` (TagMap) — smithy_query(query="operation", operation="CreateTask") lists ValidationException (HTTP 400) "The input fails to satisfy the constraints specified by an Amazon Web Services service"; the reserved-prefix / length / 50-tag rules are the service-independent AWS tagging rules the crate itself cites in src/state.rs (RESERVED_TAG_PREFIX, MAX_TAG_KEY_LENGTH, MAX_TAG_VALUE_LENGTH, MAX_TAGS) and enforces in `tag_violation` (src/state.rs lines 833-881).
Handler: src/ops/create_task.rs line 245 — `tags: input.tags.unwrap_or_default().into_iter().collect()`, with no call to `crate::state::tag_violation` anywhere in the file.
Tests: tests/all/resource_tagging.rs `tag_resource_enforces_the_tag_rules_on_a_resolved_resource` pins the rule for TagResource only (rows with `aws:cost-center`); grep for `aws:` under tests/ shows no CreateTask row, so the create path is unasserted.

## Steps to Reproduce

1. Start the emulator with an activated device in the signed region.
2. POST /task with body {"targets":["<deviceId>"],"command":{"reboot":{}},"tags":{"aws:created-by":"test"}}.
3. Observe HTTP 200 with a taskId instead of a 400 ValidationException about the reserved `aws:` prefix.
4. GET /tags/arn:aws:snow-device-management:<region>:000000000000:task/<taskId> — the reserved tag is reported as persisted.
5. Contrast: POST /tags/<same task ARN> with the same tag map is correctly refused 400 ValidationException.
6. Repeat with 60 tags, with a 200-character key, and with an empty key: all are accepted on create.
