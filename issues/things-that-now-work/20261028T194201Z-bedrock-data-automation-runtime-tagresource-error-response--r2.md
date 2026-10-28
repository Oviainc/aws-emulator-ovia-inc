Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 1fce8a5a4e99700c9d3c5ad3e1ef05753020fc4d

# Hand-applied TagKey refusal reports a position-less input path, unlike the framework's walk on the same member

_Filed: 2026-10-28T19:42:01Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `1fce8a5a4e99700c9d3c5ad3e1ef05753020fc4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The three operations that accept a tag key apply the modelled `TagKey` pattern by hand (the framework's regex engine cannot compile its `(?!aws:)` lookahead) and word the refusal through this crate's own clause/envelope helpers. The input path they pass is a hard-coded constant with NO element position: `tags.member.key` for `TagResource`/`InvokeDataAutomationAsync` and `tagKeys.member` for `UntagResource`.

That is not the path form the framework produces, and it is not the path form the OTHER half of the very same member's validation produces. `Tag$key` also carries `@length(min: 1, max: 128)`, which the boundary walk DOES enforce; when it fires, `src/service.rs` builds the clause from `violation.path`, which the framework spells with the element's 1-based position (`tags.1.member.key`). So one member, `tags[0].key`, reads back as two different paths depending only on which side caught it — exactly what `crate::state::validation_envelope`'s doc says the single-homed envelope exists to prevent. The position-less form also makes the refusal unable to say WHICH element of a 200-element `tags` list was malformed, while the walk's refusal on the same member can.

The defect is the constant (and its doc, which claims it is "the framework's own input-path form"), shared by all three tag-accepting handlers; `TagResource` is named here because it is the dedicated tagging operation that produces `tags.member.key`.

## Evidence

This crate's constants and their claim — `services/service-com-amazonaws-bedrockdataautomationruntime-amazonbedrockkeystoneruntimeservice/src/state.rs:180-186`:
```
/// The input path a `tags` list's key occupies, in the framework's own input-path form:
/// the `tags` member, the step into one element (`member`), then that element's `key`.
const TAGS_MEMBER_KEY_PATH: &str = "tags.member.key";

/// The input path a `tagKeys` list's element occupies: the list holds the keys
/// themselves, so the path stops at the step into one element.
const TAG_KEYS_MEMBER_PATH: &str = "tagKeys.member";
```
and `src/state.rs:147-148`: "`path` is the framework's own input-path form (segments joined with `.`, with `member` for the step from a list into its element)."

The framework's actual form — `vendor/smithy-schema-server/src/constraints.rs:83-86`:
```
    /// A list element names its 1-based position, then the collection step.
    pub fn element(position: usize) -> String {
        format!("{position}.{COLLECTION_STEP}")
    }
```
and its own test, `vendor/smithy-schema-server/src/constraints.rs:1876`: `assert_eq!(violations[0].path, "keySchema.2.member.keyType");` (a struct element of a list reports `<list>.<n>.member.<field>`); line 2084: `assert_eq!(violations[0].path, "names.2.member");` (a scalar element). `services/common/src/validation.rs:765-769` states the same spelling: `pub fn list_of_element(leaf_path: &str) -> Option<&str> { let head = leaf_path.strip_suffix(".member")?; let (list, position) = head.rsplit_once('.')?; ... }`.

Both halves of the SAME member reach the wire through the same helper: `src/service.rs:95` → `crate::state::violation_clause(&violation.path, &requirement)` (the walk's positional path) versus `src/state.rs:209-210` → `malformed_tag_key_refusal_at(tags.iter().map(|tag| tag.key.as_str()), TAGS_MEMBER_KEY_PATH)`, called from `src/ops/tag_resource.rs:161` (`crate::state::malformed_tag_refusal(&input.tags)`). `Tag$key` carries both constraints (`smithy_query` shape `Tag`: `"key": {"target": "TagKey", "required": true, "length": {"min": 1, "max": 128}, "pattern": "^(?!aws:)[\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]*$"}`).

The rule this breaks is stated in this crate — `src/state.rs:160-163`: "The ONE place this service's refusal envelope is written: the boundary walk's refusals reach it through `service::override_refusal`, and a rule the walk cannot express (the `TagKey` lookahead) reaches it through [`malformed_tag_key_message`], so one logical failure cannot read two different ways depending on which side caught it."

Test coverage: `tests/all/tag_key_refusal.rs:260-270` pins the position-less strings (`"tags.member.key"`, `"tags.member.key"`, `"tagKeys.member"`) and compares the three operations only to each other; no test asserts the walk's path for a tag key, so nothing pins the two forms against each other.

## Steps to Reproduce

1. Submit a job: POST / with `X-Amz-Target: AmazonBedrockKeystoneRuntimeService.InvokeDataAutomationAsync` and a valid body, keep the returned `invocationArn` (seed the referenced project first, as `tests/all/support.rs::seed_project` does).
2. POST / with `X-Amz-Target: AmazonBedrockKeystoneRuntimeService.TagResource` and body {"resourceARN": "<invocationArn>", "tags": [{"key": "<129 'a' characters>", "value": "v"}]}. The boundary walk refuses it: `1 validation error detected: Value at 'tags.1.member.key' failed to satisfy constraint: Member must have length less than or equal to 128` — the element's 1-based position is in the path.
3. POST / with the same target and body {"resourceARN": "<invocationArn>", "tags": [{"key": "aws:owner", "value": "v"}]}. The handler refuses it: `1 validation error detected: Value at 'tags.member.key' failed to satisfy constraint: Member must satisfy regular expression pattern: (?!aws:)[\p{L}\p{Z}\p{N}_.:/=+\-@]*` — same member, no position.
4. Repeat step 3 with a two-element `tags` list whose SECOND key is `aws:owner`: the message still reads `tags.member.key`, so it cannot say which element was refused.
5. Same divergence for `UntagResource` (`tagKeys.member` from the handler vs `tagKeys.1.member` from the walk) and for `InvokeDataAutomationAsync`'s `tags`.
