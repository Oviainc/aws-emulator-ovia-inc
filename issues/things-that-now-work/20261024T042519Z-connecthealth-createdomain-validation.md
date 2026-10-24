Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: be3ab9eb2c0ab303301f9b8637a58c2763937db6

# CreateDomain accepts an empty tag key, persisting a tag the model forbids

_Filed: 2026-10-24T04:25:19Z_
_Operation: CreateDomain | Class: validation_

> _Note: this issue was identified against the code base at commit `be3ab9eb2c0ab303301f9b8637a58c2763937db6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model's `TagMap` key target (`TagKey`) carries `length: min 1, max 128` (smithy_query shape=TagMap/TagKey). `CreateDomain`'s handler validates only the UPPER bounds of a tag entry (`key.len() > MAX_TAG_KEY_LEN || value.len() > MAX_TAG_VALUE_LEN`) and the entry count, never the key minimum. Because map key/value constraints are not refused by the framework's boundary walk (the handler-side checks in this crate exist precisely for that reason — see the comment in `src/ops/tag_resource.rs` and the `store_growth_is_bounded_and_refused_with_the_modelled_errors` test, where a key one past the maximum is only refused thanks to the handler's own check), a `CreateDomain` request carrying `"tags": {"": "v"}` is accepted with HTTP 201 and the empty-keyed entry is written to `store.tags_by_arn`, after which `GetDomain` and `ListTagsForResource` both report it.

Expected: a tag key shorter than 1 character is refused with HTTP 400 `ValidationException` (the standard constraint wording this crate already produces for a key one character past the maximum) and nothing is persisted.
Actual: HTTP 201, the domain is created and the invalid tag is stored and read back.

The same operation is also inconsistent with its sibling: `src/ops/tag_resource.rs` DOES check `key.is_empty()` and refuses with `ValidationException`, so the identical tag set is rejected through `TagResource` but accepted through `CreateDomain`.

## Evidence

Spec: `TagMap` key -> `TagKey` with `length {min: 1, max: 128}` (smithy_query query=shape shape=TagMap,TagKey).
Handler: `services/service-com-amazonaws-connecthealth-connecthealth/src/ops/create_domain.rs` lines ~237-264 — the `tags` guard tests only `tags.len() > MAX_TAGS_PER_RESOURCE` and `key.len() > MAX_TAG_KEY_LEN || value.len() > MAX_TAG_VALUE_LEN`; there is no `key.is_empty()` / key-minimum test. Contrast `src/ops/tag_resource.rs` lines ~211-225, which does test `key.is_empty()`.
Test coverage: none. `tests/all/domain_subscription_lifecycle.rs:934` (`store_growth_is_bounded_and_refused_with_the_modelled_errors`) exercises max count, max key length +1 and max value length +1 only; the tag-key helper `tag_map(count, key_len, value_len)` is never called with `key_len = 0`, and no test in `tests/all/resource_tagging.rs` or `tests/all/list_tags_for_resource.rs` sends an empty key to `CreateDomain`.

## Steps to Reproduce

1. POST /domain with body {"name": "clinic", "tags": {"": "value"}}
2. Observe HTTP 201 instead of HTTP 400 ValidationException naming the tag-key length constraint
3. GET /domain/{domainId} from the response and observe the returned `tags` map containing the empty key
4. GET /tags/{domainArn} (ListTagsForResource) and observe the same empty-keyed entry reported
5. For contrast, POST /tags/{domainArn} with {"tags": {"": "value"}} and observe that TagResource correctly refuses the identical entry with 400 ValidationException
