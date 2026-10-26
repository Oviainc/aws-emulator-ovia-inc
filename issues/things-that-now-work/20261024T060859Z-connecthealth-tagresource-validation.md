Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: b4572519ed5f295b6edc2ad3f0f0ced732a31b98

# TagResource measures tag key/value length in bytes, rejecting valid multi-byte tags with 400

_Filed: 2026-10-24T06:08:59Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `b4572519ed5f295b6edc2ad3f0f0ced732a31b98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model bounds a tag key (`TagKey`) at `length {min: 1, max: 128}` and a tag value (`TagValue`) at `length {min: 0, max: 256}`. Smithy's `@length` on a string counts CHARACTERS (Unicode scalar values), not bytes — the repository's own constraint checker states and implements exactly this (`vendor/smithy-schema-server/src/constraints.rs:1046-1047`: "`@length` on a string counts characters, not bytes (`chars().count()`)"), and sibling services measure tag bounds with `chars().count()` for the same reason (e.g. `services/service-com-amazonaws-codecommit-codecommit-20150413/src/state.rs:273-278`).

`TagResource`'s handler instead measures with `String::len()`, which is the UTF-8 BYTE length. Because a map key/value constraint is not refused by the framework's boundary walk (the handler's own comment says so, which is why these checks exist in the handler at all), the handler's byte-based test is the ONLY check applied.

Expected: a request whose tag key is at most 128 characters and whose tag value is at most 256 characters is accepted (HTTP 204) and the tags are stored, regardless of how many bytes those characters encode to.
Actual: any key whose UTF-8 encoding exceeds 128 bytes, or value exceeding 256 bytes, is refused with HTTP 400 `ValidationException` even though it is within the modelled character bound — e.g. a 50-character Japanese or emoji key (150-200 bytes) is rejected, and a 100-character accented value (200 bytes) is still accepted while a 130-character one (260 bytes) is not. The emulator therefore refuses input the real service accepts, and the cut-off point depends on the caller's alphabet.

The same byte-vs-character confusion is present in `CreateDomain`'s tag guard (`src/ops/create_domain.rs`, `key.len() > MAX_TAG_KEY_LEN || value.len() > MAX_TAG_VALUE_LEN`); that operation already carries an open `validation` issue (empty tag key), so this issue is filed against `TagResource`, whose guard is otherwise the stricter of the two.

## Evidence

Spec: `smithy_query(query="shape", shape="TagMap,TagKey,TagValue")` -> key target `TagKey` `length {min:1,max:128}`, value target `TagValue` `length {min:0,max:256}`. Smithy `@length` on strings counts characters, as the repo's own validator documents (`vendor/smithy-schema-server/src/constraints.rs:1046`).
Handler: `services/service-com-amazonaws-connecthealth-connecthealth/src/ops/tag_resource.rs` lines ~211-225 — `key.is_empty() || key.len() > crate::state::MAX_TAG_KEY_LEN || value.len() > crate::state::MAX_TAG_VALUE_LEN`, i.e. `str::len()` (bytes). The accompanying comment confirms this handler check is the only one applied, since "a constraint on a MAP's key or value is not one the boundary walk can refuse". `MAX_TAG_KEY_LEN`/`MAX_TAG_VALUE_LEN` in `src/state.rs:82-84` are the modelled character bounds (128 / 256).
Test coverage: none. Every tag-bound test uses ASCII only — `tests/all/resource_tagging.rs:368-371` (`"k".repeat(MAX_TAG_KEY_LEN)` and `+1`), `tests/all/list_tags_for_resource.rs:92-93`, and the `tag_map(count, key_len, value_len)` helper in `tests/all/domain_subscription_lifecycle.rs:197-205` pads with `"a"`/`"v"`. No test sends a multi-byte tag key or value, so the byte/character divergence is unasserted either way.

## Steps to Reproduce

1. POST /domain with body {"name": "clinic"} and capture `arn` from the 201 response.
2. POST /tags/{arn} with body {"tags": {"\u30bf\u30b0\u30ad\u30fc...": "v"}} where the key is exactly 128 Japanese characters (384 UTF-8 bytes).
3. Observe HTTP 400 ValidationException ("outside the bounds of a tag") instead of HTTP 204 — the key is at the modelled 128-character maximum and must be accepted.
4. POST /tags/{arn} with body {"tags": {"k": "<256 multi-byte characters>"}} and observe the same wrongful 400 for a value at the modelled 256-character maximum.
5. GET /tags/{arn} (ListTagsForResource) and observe neither tag was stored.
6. For contrast, repeat step 2 with a 128-character ASCII key and observe HTTP 204 — acceptance depends on the alphabet used, not on the modelled character bound.
