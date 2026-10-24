Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# Experiment response members not computed: state.reason is absent while initiating/running, and experimentReport is never produced for a template with a report configuration

_Filed: 2026-10-24T01:20:07Z_
_Operation: GetExperiment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

GetExperiment (and the Experiment body StartExperiment/StopExperiment return) omits two members real FIS populates:

1. `state.reason` is absent for the non-terminal statuses. The record sets `reason` only when a stop is requested or when the run settles into `completed`/`failed` (src/state.rs `settle`/`request_stop`), so an experiment observed in `initiating` or `running` answers `state: {status: "initiating"}` with no `reason`. Real FIS always pairs the status with a human-readable reason for these statuses too (the documented StartExperiment response shows `"state": {"status": "initiating", "reason": "Experiment is initiating."}`, and a running experiment likewise reports a reason). A client that reads or displays `state.reason` while polling gets a missing member for the whole non-terminal part of the lifecycle.

2. `experimentReport` is hardcoded to `None` (src/state.rs `to_experiment`, line ~1034: "`ExperimentReport` describes a report this emulator does not generate"). `experimentReportConfiguration` IS accepted, persisted and copied onto the experiment, so a caller that configures a report sees the configuration echoed but never an `experimentReport` carrying `state` (pending/running/completed/failed) or `s3Reports`, at any point of the run including after the experiment reaches `completed`. Expected: once an experiment with a report configuration finishes, `Experiment.experimentReport.state.status` is observable and `s3Reports` names where the report was written.

NOTE for the fixer: the separate omission of the PER-ACTION `ExperimentAction.state`/`startTime`/`endTime` is already tracked by the open issue 20261022T063355Z (Cause 5) and is not re-filed here.

## Evidence

Smithy model: Experiment.state -> ExperimentState {status, reason, error} — `reason` is "The reason for the state" and is not restricted to terminal statuses; Experiment.experimentReport -> ExperimentReport {state, s3Reports} and Experiment.experimentReportConfiguration -> ExperimentReportConfiguration. Well-established FIS behavior: every ExperimentState FIS reports carries a reason (documented StartExperiment response: status `initiating`, reason "Experiment is initiating."), and FIS generates an experiment report into the configured S3 output once an experiment with a report configuration ends.
Handler/state: src/ops/get_experiment.rs lines 148-159 returns `record.to_experiment()`; src/state.rs `to_state` (~995) returns `reason: self.reason.clone()` which is `None` until `settle` reaches a terminal status (~957/961) or `request_stop` runs (~990); `from_template` sets `reason: None` for `initiating` (~882); `to_experiment` sets `experiment_report: None` (~1034) unconditionally.
Tests: tests/all/experiment_run.rs asserts a reason only for the terminal `completed` state (lines 274-280) and tests/all/experiment_lifecycle.rs only for `stopped` (lines 1090-1093); no test asserts a reason while initiating/running. tests/all/experiment_lifecycle.rs line 419 only round-trips `experimentReportConfiguration`; no test reads `experimentReport`.

## Steps to Reproduce

1. Boot the emulator and point a botocore `fis` client at it.
2. create_experiment_template with a role FIS may assume, one action with a duration, and experimentReportConfiguration={'outputs': {'s3Configuration': {'bucketName': 'reports', 'prefix': 'fis'}}, 'preExperimentDuration': 'PT1M', 'postExperimentDuration': 'PT1M'}.
3. start_experiment and read the response `experiment['state']` — observe {'status': 'initiating'} with no 'reason' key.
4. get_experiment while it is running — observe {'status': 'running'} with no 'reason' key.
5. Poll get_experiment until the status is 'completed' and read experiment.get('experimentReport') — observe it is absent for every read, although experimentReportConfiguration is echoed back.
