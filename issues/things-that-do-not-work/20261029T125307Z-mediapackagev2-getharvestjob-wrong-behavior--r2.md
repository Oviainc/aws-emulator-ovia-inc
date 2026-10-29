Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: dbe80abdd81a79f1a95892fbee40e178463ee847

# A harvest job's ModifiedAt records when the job was first READ after a clock-due transition, not when the transition was due

_Filed: 2026-10-29T12:53:07Z_
_Operation: GetHarvestJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `dbe80abdd81a79f1a95892fbee40e178463ee847`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A harvest job's status changes are materialised lazily by `HarvestJobRecord::materialise_due_transitions(at)`, which every reporting and mutating harvest-job operation calls with its own `crate::state::now()` (`GetHarvestJob`, `ListHarvestJobs`, `CancelHarvestJob`). The record already holds the instant each transition became DUE: `in_progress_at` (persisted by `CreateHarvestJob`) for `QUEUED -> IN_PROGRESS`, and `terminal_at()` (computed from `schedule_configuration.end_time`) for `IN_PROGRESS -> FAILED`. The function compares `at` against those due instants to decide WHETHER the transition happened — but then commits the new status with `self.commit(at)`, i.e. it stamps `ModifiedAt` (and bumps the version) with the OBSERVATION instant rather than with the due instant it just compared against.

Consequence: `ModifiedAt` for a harvest job is a function of when a client happened to call, not of the job's own schedule. A job whose window opened at T and that nobody reads until T+1h reports `ModifiedAt = T+1h`; the same job read at T+1s reports `ModifiedAt = T+1s`. Two identical job lifecycles exercised by differently-timed clients report different modification instants for the same transition, and `ListHarvestJobs` restamps `ModifiedAt` on EVERY due job of the whole channel group with the single instant of that one list call (list_harvest_jobs.rs:205, 213-221).

This is distinct from the already-FIXED issue 20261028T234255Z-mediapackagev2-getharvestjob-wrong-behavior, which was about WHICH instant makes the `QUEUED -> IN_PROGRESS` transition due (now correctly `in_progress_at`, honouring `ScheduleConfiguration.StartTime`). The defect here is the instant that gets RECORDED as the modification time once that transition is materialised, and fixing it uses the very value that fix introduced.

## Evidence

The helper's own doc states that the transition instant is independent of reads — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/state.rs:1396-1404:
```
    /// Materialise the status changes the CLOCK already owed this job by `at`.
    ///
    /// Called by the reporting operations as well as the mutating ones, because each of
    /// these transitions was due at the same instant whether or not anybody read the job.
    /// BOTH are decided by the window the CALLER scheduled: `QUEUED` -> `IN_PROGRESS`
    /// when that window begins ([`harvest_job_in_progress_at`]) and `IN_PROGRESS` ->
    /// `FAILED` when it ends ([`HarvestJobRecord::terminal_at`], ...
```
but the code under it stamps the caller's instant — state.rs:1405-1419:
```rust
    pub fn materialise_due_transitions(&mut self, at: ::aws_smithy_types::DateTime) {
        if self.status == HARVEST_JOB_QUEUED
            && at.as_secs_f64() >= self.in_progress_at.as_secs_f64()
        {
            self.status = HARVEST_JOB_IN_PROGRESS.to_string();
            self.commit(at);
        }
        if self.status == HARVEST_JOB_IN_PROGRESS
            && at.as_secs_f64() >= self.terminal_at().as_secs_f64()
        {
            self.status = HARVEST_JOB_FAILED.to_string();
            self.error_message = Some(HARVESTER_UNAVAILABLE_MESSAGE.to_string());
            self.commit(at);
        }
    }
```
and `commit` is what writes `ModifiedAt` — state.rs:1368-1373:
```rust
    /// Commit a change. See [`ChannelGroup::commit`].
    pub fn commit(&mut self, at: ::aws_smithy_types::DateTime) {
        self.version = self.version.saturating_add(1);
        self.modified_at = at;
        self.recompute_etag();
    }
```
The due instants the function compares against are available to it: `in_progress_at` (state.rs:1431-1436 `harvest_job_in_progress_at`) and `terminal_at()` (state.rs:1389-1394).

The callers all pass the request instant, never the due instant:
- src/ops/get_harvest_job.rs:227-229:
```rust
        // Only what the clock already owed: a transition that was due at this instant
        // even if nobody had called.
        job.materialise_due_transitions(crate::state::now());
```
- src/ops/list_harvest_jobs.rs:205 `let at = crate::state::now();` then 215-221 `for job in endpoint.harvest_jobs.values_mut() { job.materialise_due_transitions(at); }`
- src/ops/cancel_harvest_job.rs:212-215 `let at = crate::state::now(); ... job.materialise_due_transitions(at);`

Test coverage: tests/all/harvest_job_scheduled_window.rs:20-22 pins only that repeated reads do not MOVE the stamp ("while the status is unchanged, repeated reads report the same `ETag` and `ModifiedAt`", asserted at lines 275-285). No test pins `ModifiedAt` to the read instant, so stamping the due instant leaves the suite's assertions satisfied.

## Steps to Reproduce

1. Create a channel group, a channel and an origin endpoint publishing HLS manifest `index`.
2. POST .../originEndpoint/<ep>/harvestJob with ScheduleConfiguration {StartTime: now + 5, EndTime: now + 3600} and HarvestedManifests {HlsManifests: [{ManifestName: "index"}]} — the job is accepted QUEUED.
3. Wait 60 seconds without calling anything, then GET .../harvestJob/<job>.
4. Observe Status IN_PROGRESS with ModifiedAt equal to the instant of THIS read (now+60), although the transition was due at StartTime (now+5), which the record holds as `in_progress_at`.
5. Repeat the same scenario but issue the first GET at now+6: ModifiedAt is now+6 instead. The same job, same schedule, different recorded modification instant purely because of when a client called.
