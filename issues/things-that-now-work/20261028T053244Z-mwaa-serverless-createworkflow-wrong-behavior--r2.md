Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 37884bada2128c91024def86659c4326af7ab1cb

# A workflow whose YAML definition declares a schedule gets no trigger unless TriggerMode is also "SCHEDULED"

_Filed: 2026-10-28T05:32:44Z_
_Operation: CreateWorkflow | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `37884bada2128c91024def86659c4326af7ab1cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents the SOURCE of a workflow's automated trigger as the scheduling information in its YAML definition: "When a workflow includes schedule information in its YAML definition, the service automatically configures the appropriate triggers for automated execution. Only one version of a workflow can have an active schedule at any given time."

The crate implements only the SECOND half of that sentence pair. `Workflow::configure_trigger` quotes it verbatim to justify re-pointing the trigger whenever the active version changes, but its body decides WHETHER there is a trigger at all from `Workflow::is_scheduled()`, which reads nothing but `snapshot.trigger_mode` and compares it to the emulator's own `TRIGGER_MODE_SCHEDULED` string. The schedule the crate parsed out of the YAML definition and froze in the same snapshot (`WorkflowSnapshot::schedule_configuration`) is never consulted by the trigger: `grep` shows it is read only by `get_workflow.rs` and `list_workflow_versions.rs`, i.e. for REPORTING.

So a workflow created from a definition object that declares `schedule: "0 12 * * *"` but carries no `TriggerMode` member (the member is optional) is stored with `next_scheduled_run_at: None`: `GetWorkflow` happily reports `ScheduleConfiguration.CronExpression = "0 12 * * *"` for it — the service demonstrably HOLDS the YAML's schedule information — while `fire_due_scheduled_runs` never owes it a firing, so `ListWorkflowRuns` stays empty forever and `RunType = SCHEDULED` is unreachable for that workflow. The mirror case is equally wrong in the other direction: a workflow with `TriggerMode = "SCHEDULED"` whose definition declares no schedule at all (or whose definition could not be snapshotted) fires a run every `SCHEDULED_RUN_EVERY`.

This is NOT the already-fixed "a workflow created with a schedule/TriggerMode never produces a run" issue (20261024T132758Z-mwaa-serverless-createworkflow-wrong-behavior): that fix built the trigger machinery and wired it to `TriggerMode`, and nothing here asks for that constant or that path to be reversed. The residual defect is the one input the documented sentence actually names — the definition's own schedule information — still being ignored by the trigger decision, even though the crate already parses it, stores it per version and reports it. The fix is confined to `Workflow::is_scheduled()`, which already has the snapshot in hand.

## Evidence

MODEL (smithy_query(query="doc", shape="GetWorkflowResponse", member="ScheduleConfiguration")): "When a workflow includes schedule information in its YAML definition, the service automatically configures the appropriate triggers for automated execution. Only one version of a workflow can have an active schedule at any given time."

CODE, the half that IS implemented — services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/state.rs:741-749 cites that very member doc and then decides the trigger from the trigger mode alone:
```
/// Called wherever the active version changes, because "only one version of a
/// workflow can have an active schedule at any given time"
/// (`smithy_query(query="shape", shape="GetWorkflowResponse")`,
/// `ScheduleConfiguration`): the trigger in force is always the latest version's,
...
pub fn configure_trigger(&mut self, now: SystemTime) {
    self.next_scheduled_run_at = self.is_scheduled().then(|| now + SCHEDULED_RUN_EVERY);
}
```
state.rs:731-735 — the predicate never looks at the definition's schedule:
```
pub fn is_scheduled(&self) -> bool {
    self.current_snapshot()
        .and_then(|snapshot| snapshot.trigger_mode.as_deref())
        .is_some_and(|mode| mode == TRIGGER_MODE_SCHEDULED)
}
```
The schedule IS parsed and stored next to it — src/state/workflow_definition.rs:230 `let schedule_configuration = declared_schedule(&content);` and src/state.rs:262-265 "The schedule the snapshotted definition declares — what `ScheduleConfiguration` reports." — and a repo-wide grep for `schedule_configuration` in `src/` finds reads ONLY at src/ops/get_workflow.rs:289 and src/ops/list_workflow_versions.rs:316 (both reporting); no trigger path reads it.

ENTRY POINT: src/ops/create_workflow.rs:437 `workflow.configure_trigger(now);` (reached again from UpdateWorkflow/version-scoped DeleteWorkflow via `promote_newest_version`, state.rs:787).

TEST COVERAGE — the case is unpinned: tests/all/workflow_definition_snapshot.rs:212-308 seeds definitions that DO declare schedules (`definition: "name: nightly\nschedule: \"0 12 * * *\"\ntasks:\n  - id: extract\n"`) and asserts only the reported `CronExpression`; `grep TriggerMode` in that file has no match, and it never looks at runs. tests/all/scheduled_workflow_runs.rs:139-143 does assert `automated: false` for "no trigger mode at all", but its `create()` helper (lines 78-96) only calls `CreateWorkflow` against `{"Bucket": "defs", "ObjectKey": "dags/scheduled.yaml"}` without ever storing that object, so `snapshot_definition` returns `None` and that row's workflow holds NO definition and therefore no schedule information — it does not pin the definition-declared case.

## Steps to Reproduce

1. Store a definition object through the emulated S3: PutObject bucket `defs`, key `dags/nightly.yaml`, body "name: nightly\nschedule: \"0 12 * * *\"\ntasks:\n  - id: extract\n".
2. Call CreateWorkflow with Name=nightly, a valid RoleArn, DefinitionS3Location={Bucket: defs, ObjectKey: dags/nightly.yaml} and NO TriggerMode member.
3. Call GetWorkflow on the returned WorkflowArn: it reports ScheduleConfiguration.CronExpression = "0 12 * * *", so the service holds the schedule information the YAML definition declares.
4. Wait past any number of firing periods and call ListWorkflowRuns for that WorkflowArn: WorkflowRuns stays [] forever, and no response anywhere reports RunType = SCHEDULED for this workflow (the stored Workflow.next_scheduled_run_at is None).
5. For contrast, repeat with TriggerMode="SCHEDULED" and a definition declaring no schedule at all: a run with RunType=SCHEDULED appears every SCHEDULED_RUN_EVERY — the trigger follows the invented TriggerMode value instead of the definition's schedule information the model names.
