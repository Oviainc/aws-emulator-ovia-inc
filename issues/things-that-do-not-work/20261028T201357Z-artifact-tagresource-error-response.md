Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: a86005fe1cb54e661d08c01cc116ff714a84de11

# The per-resource tag-ceiling ValidationException is not stated in this service's one field-validation envelope

_Filed: 2026-10-28T20:13:57Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `a86005fe1cb54e661d08c01cc116ff714a84de11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every `fieldValidationFailed` refusal this crate states itself is built through `field_validation_failed` / `field_validation_failed_over`, which renders the AWS validation envelope (`N validation error(s) detected: Value at '<member>' failed to satisfy constraint: <requirement>`) in `message` AND the same clause in the `fieldList` entry. `too_many_tags` — the refusal `TagResource` answers when a write would push a resource past `TAGS_PER_RESOURCE_LIMIT` — is the only one that bypasses that constructor: it hand-builds the `ValidationException`, sets `reason` to the same `fieldValidationFailed`, and puts a bare sentence (`"a resource holds at most 50 tags; this request would leave it with 51"`) into both `message` and `fieldList[0].message`.

So within one handler file, two refusals that both name the member `tags` and both carry `reason: fieldValidationFailed` come back worded in two different envelopes: `TagResource`'s reserved-key refusal (via `reserved_tag_keys` -> `field_validation_failed`) renders `1 validation error detected: Value at 'tags' failed to satisfy constraint: Member must not contain a tag key beginning with aws:`, while its tag-ceiling refusal renders no envelope and no clause at all. A client that parses this service's validation envelope — the form every other refusal, including the boundary-constraint path (`constraint_violation_exception`), produces — cannot parse this one.

## Evidence

The rule, stated in the crate: `services/service-com-amazonaws-artifact-artifact/src/state.rs:2467-2468` — "/// The ONE envelope every field-validation refusal of this service is stated in, over the\n/// members it names." and `state.rs:2455-2458` — "/// The one envelope every such refusal is stated in: `ValidationExceptionReason`'s\n/// `fieldValidationFailed`, the member named in `fieldList`, and the same clause in both\n/// the message and the field entry ([`field_constraint_clause`]) — the AWS validation\n/// envelope".

The sibling that follows it, `state.rs:2435-2437`:
```
pub fn reserved_tag_keys(member: &str) -> ValidationException {
    field_validation_failed(member, &reserved_tag_key_requirement())
}
```

The copy that breaks it, `state.rs:2405-2418`:
```
pub fn too_many_tags(total: usize) -> ValidationException {
    let message = format!(
        "a resource holds at most {TAGS_PER_RESOURCE_LIMIT} tags; this request would leave it \
         with {total}"
    );
    ValidationException {
        message: message.clone(),
        reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: "tags".to_string(),
            message,
        }]),
    }
}
```
Note `reason: REASON_FIELD_VALIDATION_FAILED` — it IS a field-validation refusal — yet neither `validation_envelope` (`state.rs:2492`) nor `field_constraint_clause` (`state.rs:2509`) is used.

Both call sites are in the one handler: `services/service-com-amazonaws-artifact-artifact/src/ops/tag_resource.rs:173-178` (`reserved_tag_keys("tags")`) and `tag_resource.rs:193-198` (`too_many_tags(tags.len())`).

The envelope the rest of the service answers in is pinned by `tests/all/validation_message_rendering.rs:279-288` (`"1 validation error detected: Value at '{member}' failed to satisfy constraint: {requirement}"`), `tests/all/inquiry_query_identifier_rules.rs:257` and `tests/all/compliance_inquiry_export.rs:841`. The tag-ceiling refusal is covered by `tests/all/resource_tag_writes.rs:550-602`, which asserts only `status`, `error_code`, `reason` and `fieldList[0].name` — it never asserts the message, so nothing pins the divergent wording.

## Steps to Reproduce

1. Boot the emulator and CreateComplianceInquiry to obtain an inquiry ARN.
2. POST /tags/{arn} with 50 distinct tag keys — accepted (204).
3. POST /tags/{arn} again with one further new key. Observe HTTP 400 ValidationException whose `message` is `a resource holds at most 50 tags; this request would leave it with 51` and whose `fieldList[0].message` is the same bare sentence.
4. POST /tags/{arn} with a key `aws:foo`. Observe HTTP 400 ValidationException whose `message` is `1 validation error detected: Value at 'tags' failed to satisfy constraint: Member must not contain a tag key beginning with aws:`.
5. Compare: both carry `reason: fieldValidationFailed` and both name `tags` in `fieldList`, but only the second is in the envelope `src/state.rs:2467` calls "The ONE envelope every field-validation refusal of this service is stated in".
