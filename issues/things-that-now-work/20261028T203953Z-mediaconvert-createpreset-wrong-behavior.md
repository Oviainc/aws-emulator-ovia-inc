Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 825b0e930e0bc7d005dad99ff95af4e6881702a4

# CreatePreset and GetPreset run the job-lifecycle `settle` on paths the crate's own stated rule says must not, contradicting the sibling preset/template handlers

_Filed: 2026-10-28T20:39:53Z_
_Operation: CreatePreset | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `825b0e930e0bc7d005dad99ff95af4e6881702a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate has a stated rule for when a handler may take the store's write guard and drive `ComAmazonawsMediaconvertMediaconvertState::settle` — the job-lifecycle clock sweep. `CreateJobTemplate` writes that rule out in full: "No `settle` here: this path neither reads nor writes job state, and settling is the job lifecycle's business." `ListPresets` states the same thing for the preset resource specifically: "Reporting only, and nothing about a preset is owed by the clock, so this handler takes the store to read it and writes nothing at all." `GetJobTemplate` likewise: "Reporting only: this reads the stored template and writes nothing at all."

Two preset handlers break that rule. `CreatePreset` takes a `mut` guard and calls `state.settle(now)` even though nothing it stores or projects touches job state (`PresetRecord` carries no job-derived field, and `create_preset` -> `create_named(..., |preset| preset.view(region))` reads only the preset). `GetPreset` does the same on a path the model documents as a pure read ("Retrieve the JSON for a specific preset."), and justifies it with a comment that is simply false for its own response: "Reporting only: the one write is materialising the job status changes the clock had already made due at `now`, which were owed whether or not anybody called." — `GetPreset` reports no job status at all. `ListTagsForResource` is the third instance of the same thing (a pure read that takes the write guard and settles).

The defect is that the two comments cannot both be true: either "nothing about a preset is owed by the clock" (ListPresets) or a preset read/create does owe the clock something (GetPreset/CreatePreset). A later maintainer touching the preset paths cannot tell which of the two sibling rules applies, and the five preset doors are split three-to-two on it (ListPresets, UpdatePreset, DeletePreset do not settle; CreatePreset, GetPreset do). The contrast is not cosmetic: the handlers that legitimately settle all do so for a reason visible in their own response — `GetQueue`/`ListQueues`/`CreateQueue`/`DeleteQueue` need settled job statuses because `job_counts`/`concurrency_allocation` feed the `Queue` projection or the delete's conflict guard; `GetJob`/`ListJobs`/`SearchJobs`/`GetJobsQueryResults`/`CancelJob` read or write job state directly. The two preset paths have no such reason, so they take a write lock and mutate records outside their own resource for nothing.

No behavioural impact (every job reader settles too, so no caller can observe a difference), hence LOW — but one of the two quoted comments is wrong and the rule is applied inconsistently across sibling handlers of the same resource.

## Evidence

The rule, stated explicitly — `services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/create_job_template.rs:235-237`:
```rust
        // No `settle` here: this path neither reads nor writes job state, and settling is
        // the job lifecycle's business.
        let mut state = state.lock();
```

The same rule for the preset resource — `src/ops/list_presets.rs:183-185`:
```rust
        // Reporting only, and nothing about a preset is owed by the clock, so this handler
        // takes the store to read it and writes nothing at all.
        let state = state.lock();
```

And for the job template read — `src/ops/get_job_template.rs:160-162`:
```rust
        // Reporting only: this reads the stored template and writes nothing at all.
        let state = state.lock();
```

The two handlers that break it — `src/ops/create_preset.rs:204-206` (the sibling create of `CreateJobTemplate` above, same resource shape, same absence of job state):
```rust
        let now = ::std::time::SystemTime::now();
        let mut state = state.lock();
        state.settle(now);
```

`src/ops/get_preset.rs:157-161` — the comment claims the write materialises "job status changes", but the response is a `Preset` only:
```rust
        let now = ::std::time::SystemTime::now();
        let mut state = state.lock();
        // Reporting only: the one write is materialising the job status changes the clock
        // had already made due at `now`, which were owed whether or not anybody called.
        state.settle(now);
```

That `GetPreset`'s response carries nothing job-derived — `src/ops/get_preset.rs:174-177`:
```rust
        Ok(GetPresetOutput {
            preset: Some(record.view(&region)),
        })
```
and `PresetRecord::view` reads only the record (`src/state.rs:1861-1876` lists the record's fields: `name`, `category`, `description`, `settings`, `preset_type`, `created_at`, `last_updated`, `tags` — no job state).

The third instance, a pure read that also takes the write guard — `src/ops/list_tags_for_resource.rs:192-195`:
```rust
        let mut state = state.lock();
        // Reporting only: the one write is materialising the status changes the clock had
        // already made due at `now`, which were owed whether or not anybody polled.
        state.settle(now);
```

The handlers that settle for a reason visible in their own response, for contrast — `src/ops/get_queue.rs:160-163` + `get_queue.rs:175` (`let (submitted, progressing) = state.job_counts(&name);`) and `src/ops/delete_queue.rs:153-156` ("Settle first, so the conflict guard below reads the job statuses the clock had already made due at `now`").

Smithy, for what `GetPreset` is: `smithy_query(query="doc",shape="GetPreset")` -> "Retrieve the JSON for a specific preset."

Test coverage: `tests/all/preset_lifecycle.rs`, `tests/all/preset_listing.rs`, `tests/all/preset_deletion.rs`, `tests/all/preset_update.rs` and `tests/all/resource_tagging.rs` exercise these paths end to end and assert only on the response bodies, so removing the two gratuitous `settle` calls (or adding the missing justification) leaves every existing assertion unchanged — no test observes the clock sweep through a preset or tag read.

## Steps to Reproduce

1. Read `services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/create_job_template.rs:235-237` and note the stated rule: a path that "neither reads nor writes job state" takes no `settle`.
2. Read `src/ops/list_presets.rs:183-185` and note the same rule applied to the preset resource: "nothing about a preset is owed by the clock ... writes nothing at all".
3. Read `src/ops/create_preset.rs:204-206`: the sibling create of the same kind of caller-created resource takes a `mut` guard and calls `state.settle(now)`.
4. Read `src/ops/get_preset.rs:157-161`: the preset read does the same, with a comment claiming the write materialises "job status changes" although the operation's output (`GetPresetOutput { preset }`) carries no job-derived member.
5. Compare with `src/ops/get_queue.rs:160-163` and `:175`, where the settle is needed because `job_counts` feeds the `Queue` projection — the reason that is absent in the two preset handlers.
6. Observe that the five preset doors disagree: `ListPresets`, `UpdatePreset` and `DeletePreset` do not settle; `CreatePreset` and `GetPreset` do.
