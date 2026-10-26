Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 55c71d6cc75db3eca6552dffd5fa68c4cb6ec346

# Reserved-tag-key refusal builds a ValidationException Message far past its modelled 1024-character bound

_Filed: 2026-10-26T03:41:38Z_
_Operation: CreateExport | Class: error-response_

> _Note: this issue was identified against the code base at commit `55c71d6cc75db3eca6552dffd5fa68c4cb6ec346`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::check_tag_keys_not_reserved` — the one helper `CreateExport` (through `tag_map`), `TagResource` (through `tag_map`) and `UntagResource` all refuse a reserved tag key with — clips each offending key to 128 characters but then JOINS every offender into one message. The modelled input admits 200 tag keys of 128 characters each (`ResourceTagList` is `@length 0..200`, `ResourceTagKey` is `@length 1..128`), so a request naming many distinct `aws:`-prefixed keys produces a `ValidationException.Message` of up to ~26,000 characters, while that member is `@length 0..1024`. Roughly eight distinct 128-character reserved keys already overflow the bound.

This contradicts the rule this crate states for itself: the clip constant exists specifically so a refusal stays inside `ValidationException$Message`'s modelled bound, yet this refusal is assembled from an unbounded number of clipped values and therefore breaks the very bound the clip was introduced to respect. Every other refusal of this service echoes ONE clipped value and stays inside it; this one does not.

## Evidence

Model (smithy_query shape="ValidationException"): `"Message": {"target": "GenericString", "required": true, "length": {"min": 0, "max": 1024}}`; `ResourceTagList` = `{"type": "list", "length": {"min": 0, "max": 200}}`, `ResourceTagKey` = `{"type": "string", "length": {"min": 1, "max": 128}}`.

The crate's own stated rule — services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/src/state.rs:1163-1168: "/// The most characters of a caller-supplied value a refusal message echoes." … "while `ValidationException$Message` admits only 1024 — so an echoed value is clipped to keep a refusal built from an outsized one inside the message's own modelled bound."

The code that breaks it — src/state.rs:1066-1075:
```
    let offenders = reserved
        .into_iter()
        .map(clip_for_message)
        .collect::<Vec<String>>()
        .join(", ");
    Err(ValidationException {
        message: format!(
            "{member} names tag keys in the reserved {RESERVED_TAG_KEY_PREFIX} namespace, \
             which a caller cannot set or remove: {offenders}."
```
and its own doc at src/state.rs:1049-1052: "The offenders are sorted and deduplicated … and each is clipped the way every other refusal of this service quotes a caller-supplied value" — each value is clipped, the TOTAL is not.

Reached from src/ops/create_export.rs:206 (`crate::state::tag_map(input.resource_tags.unwrap_or_default())` → src/state.rs:1100 `check_tag_keys_not_reserved(TAG_LIST_MEMBER, …)`), src/ops/tag_resource.rs:178 and src/ops/untag_resource.rs:180-184.

Test coverage: tests/all/reserved_tag_keys.rs asserts only the status, the `Fields[].Name` member and a handful of keys (one key per case) — no test pins the message length, so nothing in the crate justifies the divergence.

## Steps to Reproduce

1. POST / with X-Amz-Target: AWSBillingAndCostManagementDataExports.CreateExport and a body whose `ResourceTags` carries 20 DISTINCT reserved keys, each the 128-character string "aws:" followed by 124 'a' characters with a differing final character (e.g. "aws:aaa…a01", "aws:aaa…a02", …), plus a valid `Export` definition
2. Observe the 400 `ValidationException`: its `Message` is ~2,700 characters long (20 × 128 plus separators and the sentence)
3. Expected: a `Message` inside the modelled `GenericString` bound of 1024 characters, as src/state.rs:1163-1168 states every refusal of this service stays inside
4. Repeat against TagResource (same `ResourceTags` list) and UntagResource (the same keys in `ResourceTagKeys`) — both reach the same helper and overflow the same bound
