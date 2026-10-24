Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# UntagResource accepts tag keys the TagKey pattern forbids (reserved aws: prefix, illegal characters)

_Filed: 2026-10-24T05:11:11Z_
_Operation: UntagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UntagResource.tagKeys` targets `TagKeyList`, whose member is `TagKey` with `@pattern ^(?!aws:)[a-zA-Z+-=._:/]+$` and length 1..128. The handler checks only the list length (enforced at the boundary) and then removes each key from the resource's map.

Expected: `DELETE /tags/{resourceArn}?tagKeys=aws:owner` or `?tagKeys=my key *` is refused with HTTP 400 `ValidationException`, as the real service refuses any `tagKeys` member that fails the `TagKey` constraint (the reserved `aws:` namespace is not client-manageable).

Actual: HTTP 200 with an empty body — the illegal key is silently treated as "a key this resource does not carry" and the call reports success. The same root cause as the already-filed TagResource/CreateDatastore validation gaps (the framework's `@pattern` checker compiles model patterns with the `regex` crate, which cannot compile the `(?!…)` look-ahead, and an uncompilable pattern is treated as satisfied), but on the REMOVAL path, which neither of those issues covers: fixing them in the write paths still leaves `UntagResource` answering 200 where AWS answers 400.

## Evidence

Spec: `smithy_query(query="shape", shape="UntagResourceRequest")` → `tagKeys` targets `TagKeyList` (len 0..200); `smithy_query(query="shape", shape="TagKeyList")` → member `TagKey`, len 1..128, pattern `^(?!aws:)[a-zA-Z+-=._:/]+$`. Well-established AWS semantics: `aws:`-prefixed tag keys are service-reserved and rejected on tag and untag alike.
Handler: `src/ops/untag_resource.rs` lines 158-182 — ARN parse, `resource_tags_mut` existence check, then `for key in &input.tag_keys { tags.remove(key); }`; no character or reserved-prefix check anywhere, and no handler-side validation helper is called (contrast `src/ops/tag_resource.rs`, which at least enforces the emulator's `MAX_TAGS_PER_RESOURCE`).
Tests: `tests/all/resource_tagging.rs` lines ~249-273 cover only the `tagKeys` LIST bound (201 keys → 400 ValidationException, and 201 keys on a malformed ARN → 400 BadRequestException) and the round-trip cases use only legal keys such as `ns:sub/part.v-1`; no case supplies an `aws:`-prefixed or otherwise illegal key to `UntagResource`. Uncovered.

## Steps to Reproduce

1. CreateDatastore, then GetDatastore to read `datastoreProperties.datastoreArn`
2. POST /tags/{datastoreArn} with {"tags": {"owner": "team"}} → 200
3. DELETE /tags/{datastoreArn}?tagKeys=aws:owner → observe HTTP 200 instead of HTTP 400 ValidationException
4. DELETE /tags/{datastoreArn}?tagKeys=my%20key%20* → also HTTP 200 instead of 400
5. GET /tags/{datastoreArn} → the legal tag is still there, so the call reported success for a request the service would have refused
