Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7ebfb9678d6df6b0182d42dae67a3a93fc35190c

# TagResource stores tag keys in the reserved aws: namespace instead of refusing them

_Filed: 2026-10-24T10:08:02Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `7ebfb9678d6df6b0182d42dae67a3a93fc35190c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a TagResource call whose tags contain a key beginning with the reserved `aws:` prefix is refused (IoT SiteWise answers InvalidRequestException for it) and no tag is attached; the `aws:` namespace is reserved for AWS-generated tags and callers may not create or edit keys in it.

Actual: the only key-level check the handler runs is a length check — crate::state::check_tag_key validates 1..128 characters and nothing else (state.rs ~line 1965) — so `TagResource(resourceArn=<assetArn>, tags={"aws:owner": "x"})` answers 200 and ListTagsForResource then reports the reserved key back. The same hole lets UntagResource be used to remove an `aws:`-prefixed key.

## Evidence

Smithy model: TagKey carries only @length(min:1,max:128) and TagMap @length(min:1,max:50) — no pattern — so the reserved-prefix rule is the service's, not the model's; it is uniform, well-established AWS tagging behavior (and is filed as a gap for several other services in this backlog). Handler: src/ops/tag_resource.rs lines 171-175 (loop over keys calling check_tag_key) and state.rs check_tag_key, which returns Ok for any 1..128-character key. Tests: tests/all/resource_tags.rs covers ARN parsing, not-found and the 50-tag ceiling; grep for 'aws:' in tests finds only ARN literals, so no test asserts the reserved-prefix refusal.

## Steps to Reproduce

1. CreateAssetModel and read its assetModelArn.
2. TagResource with tags={"aws:cloudformation:stack-name": "demo"} on that ARN -> HTTP 200 where AWS refuses with InvalidRequestException.
3. ListTagsForResource on the same ARN -> the reserved key is reported as a user tag.
