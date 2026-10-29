Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: ede2de3f7102f867619bb62880127888c08c3e6a

# The tag APIs' ARN-not-found refusal is written out three times, once per tag handler

_Filed: 2026-10-29T08:34:29Z_
_Operation: TagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `ede2de3f7102f867619bb62880127888c08c3e6a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All three tag operations of this service resolve their `ResourceArn` through ONE single-homed resolution step in `state.rs` (`taggable_record`, which a fixed issue introduced precisely to stop the read path and the write path from drifting), but the REFUSAL that accompanies a failed resolution is still spelled out separately in each of the three handlers. Each copy independently decides the same three things: the message template (`"no resource with ARN '{}' exists"`), that `resource_id` is the submitted ARN, and that `resource_type` is computed from the submitted ARN with `crate::state::arn_resource_type`. Nothing else in this crate is written this way: the job-identifier not-found refusal has its text single-homed in `state.rs` (`RESOURCE_NOT_FOUND`), the client-token replay rule, the store-cap rule, the tag-admission rule, the owed-status settle rule and the ARN→tag-set resolution were each reduced to one home. The three copies can drift silently (any change to which `resource_type` a tag refusal names, or to the refusal's text, has to be made in three files), and the crate already carries a test whose whole purpose is to assert that the three copies still agree. Collapsing the three into one constructor in `state.rs` (the module that already owns `taggable_record` and `arn_resource_type`) leaves every response byte-identical and every existing test unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/ops/tag_resource.rs:167-177:
```
        let stored = state.tags_by_arn_mut(&input.resource_arn).ok_or_else(|| {
            TagResourceError::from(TagResourceErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: format!("no resource with ARN '{}' exists", input.resource_arn),
                    resource_id: input.resource_arn.clone(),
                    // The type the submitted ARN claims; the lookup spans more than one,
                    // so the refusal cannot name a fixed one.
                    resource_type: crate::state::arn_resource_type(&input.resource_arn),
                },
            ))
        })?;
```
Copy 2 — .../src/ops/untag_resource.rs:169-179:
```
        let stored = state.tags_by_arn_mut(&input.resource_arn).ok_or_else(|| {
            UntagResourceError::from(UntagResourceErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: format!("no resource with ARN '{}' exists", input.resource_arn),
                    resource_id: input.resource_arn.clone(),
                    ...
                    resource_type: crate::state::arn_resource_type(&input.resource_arn),
```
Copy 3 — .../src/ops/list_tags_for_resource.rs:156-176:
```
        let tags = state
            .tags_by_arn(&input.resource_arn)
            .cloned()
            .ok_or_else(|| {
                ListTagsForResourceError::from(
                    ListTagsForResourceErrorModeled::ResourceNotFoundException(
                        ResourceNotFoundException {
                            message: format!(
                                "no resource with ARN '{}' exists",
                                input.resource_arn
                            ),
                            resource_id: input.resource_arn.clone(),
                            // The type the submitted ARN claims; the lookup spans more
                            // than one, so the refusal cannot name a fixed one.
                            resource_type: crate::state::arn_resource_type(
                                &input.resource_arn,
                            ),
```
The LOOKUP these three refusals sit on is already single-homed, by the crate's own statement — .../src/state.rs:1413-1420: "THE one resolution step: both [`tags_by_arn`](Self::tags_by_arn) (the read `ListTagsForResource` performs) and [`tags_by_arn_mut`](Self::tags_by_arn_mut) (the write `TagResource` and `UntagResource` perform) reach their tag set through this, so the ARNs the read path resolves and the ARNs the write path resolves cannot drift apart" — and the sibling refusal text of the job-identifier lookups is single-homed too, .../src/state.rs:666: `pub const RESOURCE_NOT_FOUND: &str = "Resource is not found";` with the doc above it stating "every job-identifier lookup here refuses with it".

Test coverage that must stay unchanged (it pins exactly the agreement of the three copies): tests/all/resource_tags.rs:515-551 drives all three operations over the same unresolvable ARNs and asserts the same status, the same `error_code`, the same `resourceId` and the same `resourceType` for each — "case {case:?}/{operation}: the refusal must report the type the submitted ARN claims"; and tests/all/tag_arn_resolution.rs:267-285 asserts "an ARN no held record carries must be refused on every tag path" with `ResourceNotFoundException` for ListTagsForResource, TagResource and UntagResource.

## Steps to Reproduce

1. Open services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/ops/tag_resource.rs:167-177, .../src/ops/untag_resource.rs:169-179 and .../src/ops/list_tags_for_resource.rs:156-176 side by side: the same message template, the same `resource_id`, and the same `crate::state::arn_resource_type(&input.resource_arn)` call appear three times.
2. Compare with src/state.rs:1434-1440 (`taggable_record`), the single home of the lookup those three refusals belong to, and with src/state.rs:654-666 (`RESOURCE_NOT_FOUND`), the single home of the other not-found refusal's text.
3. Change what a tag-API refusal reports (for example the `resource_type` it names, or the message) and observe that three files must be edited in lockstep or tests/all/resource_tags.rs:539-551 fails for whichever handler was missed.
