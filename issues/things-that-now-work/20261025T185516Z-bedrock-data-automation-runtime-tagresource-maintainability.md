Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 8c118d1d43e9fad6226f410894186130d2b1ac53

# The "every tag key must satisfy the modelled TagKey pattern" refusal is written twice, character for character

_Filed: 2026-10-25T18:55:16Z_
_Operation: TagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `8c118d1d43e9fad6226f410894186130d2b1ac53`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "scan the request's `tags` for a key outside the modelled `TagKey` pattern and refuse with a `ValidationException` whose clause names the input path `tags.member.key`" is implemented twice in this crate — once in `TagResource::handle` and once in `InvokeDataAutomationAsync::handle` — as two identical blocks (only the per-operation error enum name differs). The crate already single-homes the predicate (`state::tag_key_is_well_formed`) and the wording (`state::malformed_tag_key_message`), so the remaining duplicate is the scan plus the `tags.member.key` path argument; the two copies can drift (e.g. a future change to scan nested keys, or to report every offending key, applied to one handler only). A single helper beside the two existing ones in `crate::state` (taking `&[Tag]` and returning `Option<String>`) would state it once and leave every response byte-identical.

## Evidence

src/ops/tag_resource.rs:161-174:
```
        if input
            .tags
            .iter()
            .any(|tag| !crate::state::tag_key_is_well_formed(&tag.key))
        {
            return Err(TagResourceErrorModeled::ValidationException(
                ValidationException {
                    message: Some(crate::state::malformed_tag_key_message(
                        "tags.member.key",
                    )),
                },
            )
            .into());
        }
```
src/ops/invoke_data_automation_async.rs:214-229 (same scan, same path literal, same message builder):
```
        if input
            .tags
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|tag| !crate::state::tag_key_is_well_formed(&tag.key))
        {
            return Err(InvokeDataAutomationAsyncErrorModeled::ValidationException(
                ValidationException {
                    message: Some(crate::state::malformed_tag_key_message(
                        "tags.member.key",
                    )),
                },
            )
            .into());
        }
```
(src/ops/untag_resource.rs:158-171 is the third near-copy, differing only in the `tagKeys.member` path and iterating keys.)

Both copies are covered by `tests/all/data_automation_invocations.rs:1426` — `async fn every_operation_that_accepts_a_tag_key_applies_the_modelled_tag_key_pattern()` — which asserts the same status, code and message for every operation that accepts a tag key, so single-homing the scan must leave it passing unchanged.

## Steps to Reproduce

1. Read src/ops/tag_resource.rs:161-174 and src/ops/invoke_data_automation_async.rs:214-229: the same scan, the same `tags.member.key` path literal and the same refusal construction appear in both.
2. Note that `crate::state` already owns both halves it calls (`tag_key_is_well_formed` at src/state.rs:108 and `malformed_tag_key_message` at src/state.rs:165), so there is an existing single home for the rule.
3. Run `tests/all/data_automation_invocations.rs::every_operation_that_accepts_a_tag_key_applies_the_modelled_tag_key_pattern` before and after collapsing the duplicate: responses and assertions must be unchanged.
