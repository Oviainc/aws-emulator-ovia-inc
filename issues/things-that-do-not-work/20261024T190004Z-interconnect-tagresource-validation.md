Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 38a5710b90801481080ff726caa37ba94cf84d5f

# TagResource accepts tag keys using the reserved `aws:` prefix

_Filed: 2026-10-24T19:00:04Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `38a5710b90801481080ff726caa37ba94cf84d5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AWS reserves the `aws:` tag-key prefix for AWS's own system tags: a caller may not create, edit or delete a tag whose key begins with `aws:`, and a tagging request that tries to is rejected with a validation error (nothing is written). This is uniform AWS tagging behaviour and applies to the `TagResource`/`UntagResource`/`ListTagsForResource` trio this service binds to `ConnectionResource`.

The emulator applies only two rules to an incoming `TagMap` (`crate::state::tag_write_violation`): per-entry `@length` checks for `TagKey` (1..128) and `TagValue` (0..256), and a 50-entries-per-resource cap. A key such as `aws:cloudformation:stack-name` passes both, is merged into the stored connection's tag set, and is then reported by ListTagsForResource and by `Connection$tags` on GetConnection/CreateConnection as if the caller owned it. UntagResource will likewise happily delete an `aws:`-prefixed key.

Expected: HTTP 400 with the service's modelled validation error (`InterconnectValidationException`) for any tag key beginning with `aws:` (case-insensitive), with the resource's tag set left untouched. Actual: HTTP 200 and the reserved key is persisted and returned on subsequent reads.

## Evidence

Spec/semantics grounding: the model binds TagResource with `arn` (AmazonResourceName, required) and `tags` (TagMap, required) and the service's common errors include `InterconnectValidationException` (HTTP 400), so the refusal is modelled; `TagMap` keys target `TagKey` with only a `@length(1..128)` constraint, so the reserved-prefix rule is the service-level rule the handler must add. Well-established AWS behaviour: the `aws:` prefix is reserved for AWS-assigned system tags and user tagging calls that use it fail.

Handler: services/service-com-amazonaws-interconnect-interconnect/src/ops/tag_resource.rs lines 166-230 — the only input judgement is `crate::state::tag_write_violation(None, &input.tags)` (and again after ARN resolution), then `stored.connection.tags.extend(...)`. src/state.rs `tag_write_violation` / `tag_length_violation` / `merged_tag_count` check only key/value lengths and `MAX_TAGS_PER_RESOURCE = 50`; no prefix rule exists anywhere in the crate (the same helper is the tag-on-create path in create_connection.rs, so a reserved key can also be set at create time).

Test coverage: tests/all/tag_resource.rs and tests/all/resource_tags.rs exercise length bounds, the 50-tag cap, merge-vs-replace and ARN resolution; grep for `aws:` in the tests dir finds only ARN literals — no test asserts a reserved-prefix refusal, and no existing open issue mentions it (the open TagResource-related issues cover only the error code/status for cross-account and nonexistent-connection ARNs).

## Steps to Reproduce

1. Boot the emulator and CreateConnection on a seeded environment; note the returned `arn`.
2. Call TagResource with that arn and tags {"aws:cloudformation:stack-name": "mystack"}: expect HTTP 400 InterconnectValidationException and no change to the tag set; observe HTTP 200.
3. Call ListTagsForResource with the same arn: expect the reserved key to be absent; observe it present with value "mystack".
4. Call CreateConnection with tags {"AWS:Foo": "bar"} (mixed case, same reserved prefix): expect a 400 validation refusal; observe a 200 whose `connection.tags` carries the reserved key.
