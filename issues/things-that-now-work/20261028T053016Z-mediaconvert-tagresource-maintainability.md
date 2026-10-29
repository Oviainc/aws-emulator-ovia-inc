Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 37884bada2128c91024def86659c4326af7ab1cb

# The tag-refusal classification rule is copy-pasted into five handlers instead of living in one place

_Filed: 2026-10-28T05:30:16Z_
_Operation: TagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `37884bada2128c91024def86659c4326af7ab1cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate deliberately centralises the tag-set rule and its wording in `crate::state` (`check_tags`, `tags_to_store`, `TagRefusal::message`) so that "no door onto a record's tag set can word the same refusal differently". The CLASSIFICATION half of that rule — `TagRefusal::EmptyKey` becomes the modelled `BadRequestException` (400) and `TagRefusal::PastCap` becomes the modelled `ServiceQuotaExceededException` (402) — is nevertheless written out five times, once per door, as five byte-for-byte identical `fn tag_refusal_error` bodies differing only in the generated error enum they name. Five copies of one branch can drift: a sixth door, or an edit to one copy, can map `PastCap` to 400 (or `EmptyKey` to 402) while the other four keep the documented mapping, and nothing in the crate would reconcile them. Collapsing the branch into one place in `state.rs` (the refusal already knows its own sentence there) leaves every status, code, message and existing test byte-identical.

## Evidence

The centralisation intent, quoted from the crate: services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:1478-1487 — "/// The sentence the refusal states, so no door onto a record's tag set can word the same refusal differently." and src/state.rs:1501-1512 — "Every operation that writes a record's `tags` — the four `Create*` paths and `TagResource` — is a door onto the one tag map ... so the tests a tag set takes belong to the RECORD rather than to one operation".
The five copies of the classification branch, each carrying the same doc comment ("both worded by [`crate::state::TagRefusal`] so every door onto a record's tags refuses alike"):
- src/ops/tag_resource.rs:220-232 — `fn tag_refusal_error(refusal: crate::state::TagRefusal) -> TagResourceErrorModeled { ... TagRefusal::EmptyKey => ...BadRequestException..., TagRefusal::PastCap => ...ServiceQuotaExceededException... }`
- src/ops/create_job.rs:635-647 — same body, `-> CreateJobErrorModeled`
- src/ops/create_job_template.rs:381-393 — same body, `-> CreateJobTemplateErrorModeled`
- src/ops/create_preset.rs:263-275 — same body, `-> CreatePresetErrorModeled`
- src/ops/create_queue.rs:357-369 — same body, `-> CreateQueueErrorModeled`
Tests that pin the responses the fix must not change: tests/all/resource_tagging.rs:389-420 (`TagOutcome::BadRequest => 400/"BadRequestException"`, `TagOutcome::Quota => 402/"ServiceQuotaExceededException"`) driven over every door including the four `Create*` paths, with cases at tests/all/resource_tagging.rs:1006-1065; tests/all/job_template_crud.rs:1217 pins the 402 code as well.

## Steps to Reproduce

1. grep -n 'fn tag_refusal_error' services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops — five hits, in tag_resource.rs, create_job.rs, create_job_template.rs, create_preset.rs and create_queue.rs.
2. Diff any two of the five bodies: they are identical apart from the generated error enum name, so the EmptyKey->400 / PastCap->402 rule exists in five places.
3. Edit one copy (e.g. map PastCap to BadRequestException in create_preset.rs) and observe that nothing in the crate detects the divergence from the other four doors: only tests/all/resource_tagging.rs's preset case fails, and a newly added door would carry no check at all.
