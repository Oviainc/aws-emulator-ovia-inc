Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 653ea257673c095df18531588ef6bfc9ad1ec3b6

# Reserved-tag-key refusal echoes every offending key, so ValidationException.Message can exceed its modelled 1024-character bound

_Filed: 2026-10-26T07:52:11Z_
_Operation: UntagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `653ea257673c095df18531588ef6bfc9ad1ec3b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`check_tag_keys_not_reserved` builds its `ValidationException.Message` by joining EVERY offending tag key into the sentence. Each key is passed through `clip_for_message`, but that clip is 128 characters and a `ResourceTagKey` is itself at most 128 characters, so the clip is a no-op per key and does nothing to bound the TOTAL.

`UntagResource.ResourceTagKeys` is a list of up to 200 keys of up to 128 characters each, so a single request naming many reserved keys produces a `Message` of up to roughly 26 000 characters — against `ValidationException$Message`'s modelled `@length(0, 1024)`. Nine reserved keys at the key length ceiling already pushes the message past 1024.

This breaks the rule the crate states for itself in exactly this place: `MESSAGE_VALUE_CLIP` exists, by its own doc comment, precisely "to keep a refusal built from an outsized one inside the message's own modelled bound" — and the crate's tests assert that bound for other refusal families, so the aggregate path is the one that escapes it. The same helper is reached from `CreateExport` and `TagResource` through `tag_map` with up to 200 `ResourceTags`, so all three tag-accepting operations can emit the oversized body; `UntagResource` is the direct caller.

## Evidence

The crate's own stated rule — services/.../src/state.rs:1163-1177:

```
1163 /// The most characters of a caller-supplied value a refusal message echoes.
1164 ///
1165 /// `TableName` admits 1024 characters and a table property value 16384 (smithy_query
1166 /// shapes `TableName`, `TableProperties`), while `ValidationException$Message` admits
1167 /// only 1024 — so an echoed value is clipped to keep a refusal built from an outsized
1168 /// one inside the message's own modelled bound.
1169 const MESSAGE_VALUE_CLIP: usize = 128;
...
1173 pub(crate) fn clip_for_message(value: &str) -> String {
1174     if value.chars().count() <= MESSAGE_VALUE_CLIP {
1175         return value.to_string();
```

The code that breaks it — services/.../src/state.rs:1066-1075:

```
1066     let offenders = reserved
1067         .into_iter()
1068         .map(clip_for_message)
1069         .collect::<Vec<String>>()
1070         .join(", ");
1071     Err(ValidationException {
1072         message: format!(
1073             "{member} names tag keys in the reserved {RESERVED_TAG_KEY_PREFIX} namespace, \
1074              which a caller cannot set or remove: {offenders}."
1075         ),
```

The bound and the reachable input size, from the model: `smithy_query(query="shape", shape="ValidationException")` -> `"Message": {"target": "GenericString", "required": true, "length": {"min": 0, "max": 1024}}`; `smithy_query(query="shape", shape="ResourceTagKey")` -> `"length": {"min": 1, "max": 128}`; OPERATIONS.md:70 for UntagResource — "`ResourceTagKeys` (list<string>, len 0..200, required)". 200 x 128 + separators is ~26 000 characters, so the no-op per-key clip cannot keep the message inside 1024.

The handler path — services/.../src/ops/untag_resource.rs:180-184:

```
180         crate::state::check_tag_keys_not_reserved(
181             crate::state::TAG_KEY_LIST_MEMBER,
182             input.resource_tag_keys.iter().map(String::as_str),
183         )
184         .map_err(UntagResourceErrorModeled::ValidationException)?;
```

Test coverage: the 1024 bound IS asserted elsewhere in this crate — tests/all/destination_configuration.rs:53-55 ("`ValidationException$Message`'s modelled `@length 0..1024`, which a refusal built from ..."), tests/all/table_catalog.rs:620-621 `!message.is_empty() && message.len() <= 1024`, tests/all/validation_message_rendering.rs:307-308 `message.len() <= 1024` — but tests/all/reserved_tag_keys.rs only ever sends one or two keys per request (reserved_tag_keys.rs:191 `json!({ "ResourceArn": arn, "ResourceTagKeys": [key] })` and lines 203-213 with two), so nothing pins the aggregate case.

## Steps to Reproduce

1. Create an export and note its ARN (POST / with `X-Amz-Target: AWSBillingAndCostManagementDataExports.CreateExport`).
2. Call UntagResource with `{"ResourceArn": "<arn>", "ResourceTagKeys": [ ...200 distinct keys, each "aws:" followed by 124 characters... ]}` — all inside the modelled bounds (`ResourceTagKeys` len 0..200, each key len 1..128), so boundary validation passes and the handler runs.
3. Observe the HTTP 400 `ValidationException` body: its `Message` echoes all 200 keys and is tens of thousands of characters long, against the modelled `@length(0, 1024)` on `ValidationException$Message`.
4. Repeat with only 9 reserved keys of 128 characters each: the message already exceeds 1024 characters.
5. Same via TagResource `{"ResourceArn": "<arn>", "ResourceTags": [{"Key": "aws:...", "Value": "x"}, ...]}` and via CreateExport's `ResourceTags`, which reach the same helper through `crate::state::tag_map` (state.rs:1100).
