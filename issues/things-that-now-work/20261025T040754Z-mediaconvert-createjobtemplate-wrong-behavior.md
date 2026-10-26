Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0c701d5f8ae664c771f5c5cdfd2b2b982b891619

# CreateJobTemplate/UpdateJobTemplate store a blank Category or Description verbatim, unlike the preset handlers that normalise it to absent

_Filed: 2026-10-25T04:07:54Z_
_Operation: CreateJobTemplate | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0c701d5f8ae664c771f5c5cdfd2b2b982b891619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Category` and `Description` are the same kind of optional free text on a job template as on a preset. The preset handlers normalise a value that carries no characters to ABSENT, and state the rule in a comment: an empty string carries no more information than absence, so it must not be stored as a member a later read echoes back blank. `CreateJobTemplate` moves `input.category`/`input.description` straight onto the record, and `UpdateJobTemplate` likewise assigns `Some(category)`/`Some(description)` unconditionally.

Consequences inside this crate:
- `GetJobTemplate`/`ListJobTemplates` echo `"category": ""` for a template created with a blank category, where the equivalent preset reports `null` (`JobTemplateRecord::view`, src/state.rs:1703-1706, projects the stored `Option` as-is);
- a blank value can never be CLEARED on a job template: `UpdateJobTemplate` with `"description": ""` stores `Some("")`, while `UpdatePreset` with the same body clears the member (pinned by a test);
- the listing filters are written against the assumption that a record with no category matches no category a caller can name (list_job_templates.rs:194-204, list_presets.rs:206-209) — a template stored with `Some("")` is a record that matches the empty `category=` query value, a state no preset can be in.

## Evidence

Sibling handlers apply the normalisation and state the rule — services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/create_preset.rs:173-177:
```
// `Category` and `Description` are optional free text; an empty string carries no
// more information than their absence, so it is stored as absent rather than as a
// member a later read would echo back blank.
let category = input.category.filter(|value| !value.trim().is_empty());
let description = input.description.filter(|value| !value.trim().is_empty());
```
and src/ops/update_preset.rs:194-199:
```
if let Some(category) = input.category {
    record.category = Some(category).filter(|value| !value.trim().is_empty());
}
if let Some(description) = input.description {
    record.description = Some(description).filter(|value| !value.trim().is_empty());
}
```

This handler skips it — src/ops/create_job_template.rs:281-284:
```
let record = crate::state::JobTemplateRecord {
    name,
    category: input.category,
    description: input.description,
```
and the update path, src/ops/update_job_template.rs:284-289:
```
if let Some(category) = input.category {
    template.category = Some(category);
}
if let Some(description) = input.description {
    template.description = Some(description);
}
```

Test coverage: tests/all/preset_update.rs:191-200 pins the preset rule ("an empty category clears it, as an empty one does on create" → expected stored category `None`; "a blank description clears it and leaves the category alone"). tests/all/job_template_crud.rs exercises only non-blank categories (lines 131-140, 603-605, 667-668), so nothing pins the job-template behaviour.

## Steps to Reproduce

1. Boot the emulator and point a mediaconvert client at it.
2. create_job_template(Name='t', Category='', Description='   ', Settings={...one output group with a container...}).
3. get_job_template(Name='t') — observe JobTemplate.Category == '' and JobTemplate.Description == '   ' echoed back blank.
4. Positive control: create_preset(Name='p', Category='', Settings={...}) then get_preset(Name='p') — Category is absent (null).
5. update_job_template(Name='t', Description='') — observe the blank value stored instead of the member being cleared, whereas update_preset(Name='p', Category='') clears it.
