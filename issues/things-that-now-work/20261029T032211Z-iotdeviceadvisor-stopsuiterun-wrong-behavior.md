Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f75cbb9a461527dc7539f53b0560573b007d83b1

# Stopping a PENDING run leaves startTime unset forever, so the STOPPED run reports an endTime with no startTime

_Filed: 2026-10-29T03:22:11Z_
_Operation: StopSuiteRun | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f75cbb9a461527dc7539f53b0560573b007d83b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StopSuiteRun` moves a run out of `PENDING` straight into `STOPPING` (and from there the clock moves it to `STOPPED` with an `endAt`), but it never stamps the run's `started_at`. `started_at` is written in exactly ONE place in the crate — the `PENDING -> RUNNING` arm of `SuiteRun::materialize_due` — so a run that is stopped while still queued skips that arm and keeps `started_at == None` for the rest of its life.

That breaks the invariant the field's own doc comment in this crate states ("Set when the run leaves `PENDING`"): the run HAS left `PENDING` (it is `STOPPING`, then `STOPPED`, and `SuiteRun::is_in_progress`/`is_stoppable` both treat it as no longer queued), yet the member is absent. The observable consequence is on the two readers that project it: `GetSuiteRun` answers `status: "STOPPED"` and a numeric `endTime` with `startTime` absent, and `ListSuiteRuns` answers the same entry with `endAt` set and `startedAt` null — a run that ended without ever having started. A client computing a run's duration from `startTime`/`endTime` gets a half-populated pair for every run stopped before its first transition became due, which is the normal case: `StopSuiteRun` issued within `STATUS_TRANSITION` of `StartSuiteRun` always hits the `PENDING` run.

The fix is for the stop path to materialise the start stamp it skipped (the same way the `PENDING -> RUNNING` arm does) rather than leaving the field permanently unset; it must not touch a run that is already `RUNNING` (that one already carries its own stamp) nor a terminal run (`is_stoppable()` already excludes both).

## Evidence

The invariant the crate states for the field — services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/state.rs:331-332:
```
    /// Set when the run leaves `PENDING`; absent while it is still queued.
    pub started_at: Option<SystemTime>,
```
The ONLY writer of that field (confirmed by grep: `started_at` is assigned nowhere else in `src/`) — src/state.rs:361-366:
```
            match self.status.as_str() {
                "PENDING" => {
                    self.status = "RUNNING".to_string();
                    self.started_at = Some(due);
                    self.next_transition_at = Some(due + STATUS_TRANSITION);
                }
```
The path that breaks it — src/ops/stop_suite_run.rs:172-180 (a `PENDING` run is stoppable, so it leaves `PENDING` here without a start stamp):
```
        // Only a run that has not been asked to stop and has not finished is moved: a
        // second stop neither restarts the stop clock nor resurrects a finished run.
        let outcome = if run.is_stoppable() {
            run.status = "STOPPING".to_string();
            run.next_transition_at = Some(now + crate::state::STATUS_TRANSITION);
            "stopping"
        } else {
            "unchanged-already-stopping-or-finished"
        };
```
`is_stoppable` includes `PENDING` — src/state.rs:407-409:
```
    pub fn is_stoppable(&self) -> bool {
        matches!(self.status.as_str(), "PENDING" | "RUNNING")
    }
```
The `STOPPING -> STOPPED` arm stamps only `end_at`, never the missing start — src/state.rs:373-377:
```
                "STOPPING" => {
                    self.status = "STOPPED".to_string();
                    self.end_at = Some(due);
                    self.next_transition_at = None;
                }
```
What the readers then answer — src/ops/get_suite_run.rs:185-186 and src/ops/list_suite_runs.rs:250-251:
```
            start_time: run.started_at.map(Into::into),
            end_time: run.end_at.map(Into::into),
```
```
                started_at: run.started_at.map(Into::into),
                end_at: run.end_at.map(Into::into),
```
The crate's own test states the rule for the other transition — tests/all/suite_lifecycle.rs:1072-1076:
```
        assert!(
            running.member("startTime").is_number(),
            "case {case:?}: a run that left PENDING carries a startTime, got {}",
            running.body
        );
```
and the crate's own fixture encodes the same invariant — tests/all/suite_run_report.rs:101:
```
            started_at: (status != "PENDING").then_some(created_at),
```
Test coverage: the stop path is exercised by `stopping_a_run_is_observable_and_never_resurrects_a_terminal_run` (tests/all/suite_lifecycle.rs:1128-1242), which stops the run immediately after `StartSuiteRun` (i.e. while it is `PENDING`) and asserts only `status` and `endTime` (:1193-1204) — it never looks at `startTime`, so this behaviour is UNTESTED rather than pinned with a stated reason.

## Steps to Reproduce

1. Start the emulator and create a suite definition with CreateSuiteDefinition (`POST /suiteDefinitions`) carrying a legal `suiteDefinitionConfiguration`.
2. Start a run: `POST /suiteDefinitions/{suiteDefinitionId}/suiteRuns` with `suiteRunConfiguration.primaryDevice.thingArn` set. The run is stored as `PENDING` (`started_at: None`, start_suite_run.rs:318).
3. Immediately (within `state::STATUS_TRANSITION`, 500 ms) call `POST /suiteDefinitions/{suiteDefinitionId}/suiteRuns/{suiteRunId}/stop`.
4. Wait out one transition span and call `GET /suiteDefinitions/{suiteDefinitionId}/suiteRuns/{suiteRunId}`. Observed: `"status": "STOPPED"` and a numeric `endTime`, but NO `startTime` member. Expected per state.rs:331 ("Set when the run leaves `PENDING`") and the sibling assertion at tests/all/suite_lifecycle.rs:1072-1076: a run that left `PENDING` carries a `startTime`.
5. Call `GET /suiteRuns?suiteDefinitionId={suiteDefinitionId}` and observe the same entry with `endAt` set and `startedAt` null.
