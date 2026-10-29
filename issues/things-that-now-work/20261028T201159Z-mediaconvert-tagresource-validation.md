Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: db5b8cf4b40010fa5fa0137914026d79724270f1

# TagResource/UntagResource modify built-in SYSTEM queues, presets and job templates that every other write door in the crate refuses

_Filed: 2026-10-28T20:11:59Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `db5b8cf4b40010fa5fa0137914026d79724270f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` and `UntagResource` resolve the request's `Arn` to a `TaggedResource` and then write straight through `state.tags_mut(&resource)`, which hands back `&mut` on the record's own tag map for ANY stored record — including the records the store seeds as built-in: the `Default` queue (`queue_type = QUEUE_TYPE_SYSTEM`), the eleven `SYSTEM_PRESETS` and the twelve `SYSTEM_JOB_TEMPLATES`.

Every OTHER door in this crate that writes one of those records refuses it: `UpdateQueue`, `UpdatePreset` and `UpdateJobTemplate` each test the record's type and answer the modelled `ForbiddenException` worded by the crate's own `crate::state::system_resource_modify_message("name")`; the three deletes answer `system_resource_delete_message`; `create_named` answers `CreateOutcome::SystemResource` so a create under a built-in name is a `ConflictException`. The two tagging doors are the only writers that skip the test, so `POST /2017-08-29/tags` with `arn:aws:mediaconvert:<region>:<account>:queues/Default` answers 200 and permanently changes a record the crate's own state-module doc says "no request path removes or re-seeds", and `ListTagsForResource` then reports the caller's tag on it. `UntagResource` on the same ARN removes it again — the same unguarded `tags_mut` path.

The crate also already treats a record's tag set as part of the record rather than a side table (`state.rs` doc on `ComAmazonawsMediaconvertMediaconvertState::tags`: "Tags are not a map of their own: they live ON the record they belong to"), so a tag write IS a modification of the built-in resource. Nothing in the crate or its tests justifies the exception: `tests/all/resource_tagging.rs` contains no `System`/`Default` case at all (grep for `System|Default|system` in that file finds nothing), so no existing assertion covers or pins this.

## Evidence

The unguarded write — `services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/tag_resource.rs:207-221`:
```rust
        let Some(tags) = state.tags_mut(&resource) else {
            return Err(TagResourceErrorModeled::NotFoundException(NotFoundException {
                message: Some(format!("Resource {} not found.", input.arn)),
            })
            .into());
        };

        // The merge is computed before anything is written, so a call refused for growing
        // past the tag cap leaves the resource's tags exactly as they were.
        let mut merged = tags.clone();
        merged.extend(requested);
        if let Err(refusal) = crate::state::check_tags(&merged) {
            return Err(tag_refusal_error(refusal).into());
        }
        *tags = merged;
```
The same path in `src/ops/untag_resource.rs:193-202`:
```rust
        let Some(tags) = state.tags_mut(&resource) else {
            return Err(UntagResourceErrorModeled::NotFoundException(NotFoundException {
                message: Some(format!("Resource {} not found.", input.arn)),
            })
            .into());
        };
        for key in input.tag_keys.unwrap_or_default() {
            tags.remove(&key);
        }
```
Neither file contains `is_system`, `TYPE_SYSTEM`, `QUEUE_TYPE_SYSTEM` or `system_resource_modify_message` anywhere (grep over `src/ops` for those four symbols matches only `update_queue.rs`, `update_preset.rs` and `update_job_template.rs`).

The door it reaches is type-blind — `src/state.rs:4898-4912`:
```rust
    pub fn tags_mut(
        &mut self,
        resource: &TaggedResource,
    ) -> Option<&mut BTreeMap<String, String>> {
        match resource {
            TaggedResource::Queue(name) => self.queues.get_mut(name).map(|queue| &mut queue.tags),
```

The sibling write doors that DO apply the test — `src/ops/update_queue.rs:213-222`:
```rust
        // `Queue$Type`: "System queues are built in. You can't modify or delete system
        // queues." A built-in queue is not "one of your existing queues", so the
        // modification is refused the way `UpdateJobTemplate` and `UpdatePreset` refuse a
        // built-in template or preset.
        if record.queue_type == crate::state::QUEUE_TYPE_SYSTEM {
            return Err(UpdateQueueErrorModeled::ForbiddenException(ForbiddenException {
                message: Some(crate::state::system_resource_modify_message("name")),
            })
            .into());
        }
```
and `src/ops/update_preset.rs:186-194`:
```rust
        // `Type`: "A preset can be of two types: system or custom", and a built-in one is
        // not the caller's to change — the same refusal `UpdateJobTemplate` gives for a
        // `SYSTEM` template.
        if record.preset_type == crate::state::TYPE_SYSTEM {
            return Err(UpdatePresetErrorModeled::ForbiddenException(ForbiddenException {
                message: Some(crate::state::system_resource_modify_message("name")),
            })
```
(`src/ops/update_job_template.rs:254-259` is the third copy.)

The crate's own statement of the rule — `src/state.rs:27-32`:
```
//!   either. The built-in system templates `job_templates` is seeded with, and the
//!   built-in system presets `presets` is seeded with, are
//!   inside those caps, and no request path removes or re-seeds them: `UpdateJobTemplate`
//!   and `DeleteJobTemplate` refuse a `SYSTEM` template, and `UpdatePreset` and
//!   `DeletePreset` a `SYSTEM` preset, with the modelled
//!   `ForbiddenException`.
```
and `src/state.rs:649-655`, which names tagging's own kind of change:
```
/// As [`system_resource_delete_message`], for a request that would MODIFY a built-in
/// resource rather than delete it — `UpdateJobTemplate` on a `SYSTEM` template, which
/// `JobTemplate$Type` says a user "can't modify or delete".
```

Test coverage: `tests/all/resource_tagging.rs` exercises created queues/jobs/presets/templates and absent/malformed ARNs only — it has no case naming a built-in resource, so no existing assertion changes.

## Steps to Reproduce

1. Start the emulator with a fresh MediaConvert state (the store seeds the SYSTEM `Default` queue, 11 SYSTEM presets and 12 SYSTEM job templates).
2. GET /2017-08-29/queues/Default and note `"type": "SYSTEM"` in the response.
3. PUT /2017-08-29/queues/Default with {"description":"mine"} — observe HTTP 403 ForbiddenException, message "You specified an invalid value for name. You are not allowed to modify system resources."
4. POST /2017-08-29/tags with {"arn":"arn:aws:mediaconvert:us-east-1:000000000000:queues/Default","tags":{"owner":"me"}} — observe HTTP 200 and an empty body instead of the 403 the previous step gave for modifying the same built-in record.
5. GET /2017-08-29/tags/arn%3Aaws%3Amediaconvert%3Aus-east-1%3A000000000000%3Aqueues%2FDefault — the built-in queue now reports the caller's tag {"owner":"me"}.
6. Repeat step 4 against arn:...:presets/System-Generic_Hd_Mp4_Avc_Aac_16x9_1920x1080p_24Hz_6Mbps and arn:...:jobTemplates/System-Ott_Hls_Ts_Avc_Aac — both answer 200, while PUT /2017-08-29/presets/<same name> and PUT /2017-08-29/jobTemplates/<same name> answer 403.
7. PUT /2017-08-29/tags/<the encoded Default queue ARN> with {"tagKeys":["owner"]} — HTTP 200, the built-in record is modified again.
