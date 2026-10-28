Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9f84f44c6767c5c874b7cc6e16fb6469b8a0f678

# A harvest job is reported IN_PROGRESS 400 ms after creation regardless of its scheduled StartTime, while the same rule honours EndTime

_Filed: 2026-10-28T23:42:55Z_
_Operation: GetHarvestJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9f84f44c6767c5c874b7cc6e16fb6469b8a0f678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A harvest job's lifecycle is driven by `HarvestJobRecord::materialise_due_transitions`, which every reporting operation calls (`GetHarvestJob`, `ListHarvestJobs`, `CancelHarvestJob`). That function decides the terminal `IN_PROGRESS` -> `FAILED` transition from the window the CALLER scheduled (`self.schedule_configuration.end_time`), but decides the `QUEUED` -> `IN_PROGRESS` transition from a fixed offset after the instant the job was CREATED (`in_progress_at = created_at + HARVEST_JOB_QUEUED_MILLIS`), which `CreateHarvestJob` computes without ever consulting `ScheduleConfiguration.StartTime`.

So one end of the caller's scheduled window governs the job and the other end is ignored:

* a job scheduled to run in an hour (`StartTime = now + 3600`) is reported `IN_PROGRESS` ~0.4 s after it was created, an hour before the window it was scheduled for begins;
* a job whose whole window is already in the past (`StartTime = now - 7200`, `EndTime = now - 3600` — accepted, because `CreateHarvestJob` only refuses `end < start` and `end == start`) is walked through BOTH transitions inside a single read: the first `GetHarvestJob` commits `IN_PROGRESS` and then immediately commits `FAILED` with the harvester-unavailable message, bumping the version twice and rotating the ETag twice in one reporting call.

`start_time` is read nowhere else in the crate: `grep start_time src/` finds it only in `create_harvest_job.rs:269` (the end-vs-start ordering check) and in unrelated DASH members, so no code path ever lets the scheduled start decide when the job starts.

## Evidence

The model states that the schedule is when the job runs:
* `HarvestJob$ScheduleConfiguration` (smithy_query shape=HarvestJob): "The configuration for when the harvest job is scheduled to run."
* `CreateHarvestJobRequest$ScheduleConfiguration`: "The configuration for when the harvest job should run, including start and end times."
* `HarvesterScheduleConfiguration$StartTime` (@required): "The start time for the harvest job."

The crate honours only the end of that window — both halves of ONE function, services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/state.rs:1239-1252:
```rust
    pub fn materialise_due_transitions(&mut self, at: ::aws_smithy_types::DateTime) {
        if self.status == HARVEST_JOB_QUEUED
            && at.as_secs_f64() >= self.in_progress_at.as_secs_f64()
        {
            self.status = HARVEST_JOB_IN_PROGRESS.to_string();
            self.commit(at);
        }
        if self.status == HARVEST_JOB_IN_PROGRESS
            && at.as_secs_f64() >= self.schedule_configuration.end_time.as_secs_f64()
        {
            self.status = HARVEST_JOB_FAILED.to_string();
```
and its own doc comment states the asymmetry (state.rs:1236-1238):
```
    /// `QUEUED` -> `IN_PROGRESS` once [`HARVEST_JOB_QUEUED_MILLIS`] has passed, and
    /// `IN_PROGRESS` -> `FAILED` once the window the CALLER scheduled has ended
```
The instant it compares against is a function of the creation time alone — state.rs:1256-1262:
```rust
pub fn harvest_job_in_progress_at(
    created_at: ::aws_smithy_types::DateTime,
) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs_f64(
        created_at.as_secs_f64() + (HARVEST_JOB_QUEUED_MILLIS as f64 / 1000.0),
    )
```
and that is what the create stores — src/ops/create_harvest_job.rs:435:
```rust
            in_progress_at: crate::state::harvest_job_in_progress_at(created_at),
```
while the reader materialises against it — src/ops/get_harvest_job.rs:226-228:
```rust
        // Only what the clock already owed: a transition that was due at this instant
        // even if nobody had called.
        job.materialise_due_transitions(crate::state::now());
```

Test coverage: no test pins the start side. `tests/all/rejection_rule_and_negative_paths.rs:1264` builds every job with `harvest_body(start, start + 600.0, manifests)` (a window that starts now), and `tests/all/harvest_job_idempotency.rs:354-359` only varies `StartTime` to prove the idempotency fingerprint changes ("the window starts later"), never the status it produces. `tests/all/shared_entity_tag_precondition.rs:214-229` polls "until the `QUEUED` -> `IN_PROGRESS` transition the clock owes" for a job whose window starts now, so honouring `StartTime` leaves it unchanged.

## Steps to Reproduce

1. Create a channel group, a CMAF channel and an origin endpoint publishing manifest `index`.
2. POST .../originEndpoint/<ep>/harvestJob with ScheduleConfiguration {StartTime: now + 3600, EndTime: now + 7200} and HarvestedManifests {HlsManifests: [{ManifestName: "index"}]} — the create answers 200 with Status QUEUED.
3. Wait ~1 second and GET .../harvestJob/<job>: Status is reported as IN_PROGRESS, although the window the caller scheduled does not begin for another hour.
4. Repeat with ScheduleConfiguration {StartTime: now - 7200, EndTime: now - 3600} (accepted, since EndTime > StartTime): the first GET reports FAILED with the harvester-unavailable ErrorMessage, having committed two versions of the job (ETag rotated twice) inside one read.
