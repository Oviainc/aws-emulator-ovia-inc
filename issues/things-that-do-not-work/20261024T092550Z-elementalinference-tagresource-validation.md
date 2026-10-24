Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 19809ff85dacb8a5011e45ecfee3e76273a4f2a9

# TagResource accepts tag keys in the reserved aws: namespace instead of refusing them

_Filed: 2026-10-24T09:25:50Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `19809ff85dacb8a5011e45ecfee3e76273a4f2a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AWS reserves the `aws:` tag-key prefix for AWS-generated tags: a user request that supplies a key beginning with `aws:` (any case) is refused with a 400 validation error and nothing is stored.

The emulator's TagResource validates only what the modeled `TagMap` bounds plus its own per-resource cap: each key's length against `TAG_KEY_MIN_LENGTH..=TAG_KEY_MAX_LENGTH`, and the resulting tag count against `MAX_TAGS_PER_RESOURCE` (50). The reserved prefix is never inspected, so `{"aws:cloudformation:stack-name": "mine"}` is merged into the stored resource and is then reported by every later read of the feed/dictionary (`GetFeed$tags`, `GetDictionary$tags`, and `ListTagsForResource` once that stub is implemented).

Expected: HTTP 400 `ValidationException` (modeled for TagResource) and no tag written. Actual: HTTP 204 with the reserved key persisted.

## Evidence

Spec: `ValidationException` (HTTP 400) is modeled for TagResource; the modeled `TagMap` constrains only key length (1..128) and value length (..256), so a service-side rule can only live in the handler. The `aws:` reserved-prefix rule is well-established AWS tagging semantics (the same gap is tracked for other services in this corpus, e.g. bcm-dashboards/rbin/compute-optimizer TagResource validation issues), and it is the same rule the open CreateDictionary issue (`20261024T012105Z-elementalinference-createdictionary-validation`) raises for create-time tags — that issue covers CreateDictionary/CreateFeed only, not this operation.
Handler: `src/ops/tag_resource.rs` `handle()` — the only input checks are the per-key length window (`crate::state::TAG_KEY_MIN_LENGTH..=TAG_KEY_MAX_LENGTH`) and the resulting-count check against `crate::state::MAX_TAGS_PER_RESOURCE`; the merge loop `for (key, value) in input.tags { tags.insert(key, value); }` writes whatever key arrived.
Tests: `tests/all/resource_tagging.rs` covers ARN resolution, the key-length refusal and the 50-tag cap; no case supplies an `aws:`-prefixed key.

## Steps to Reproduce

1. POST /v1/feed with one output -> note the feed id and ARN
2. POST /v1/tags/{feedArn} with {"tags": {"aws:cloudformation:stack-name": "mine"}} -> observe HTTP 204 instead of HTTP 400 ValidationException
3. GET /v1/feed/{id} -> observe the reserved `aws:cloudformation:stack-name` key echoed in `tags`
4. Repeat against a dictionary ARN for the same result
