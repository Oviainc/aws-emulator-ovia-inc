Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# A workflow created with a schedule/TriggerMode never produces a run, so RunType=SCHEDULED is unreachable

_Filed: 2026-10-24T13:27:58Z_
_Operation: CreateWorkflow | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a workflow whose definition/TriggerMode asks for automated execution is actually triggered by the service — 'Amazon Managed Workflows for Apache Airflow Serverless integrates with EventBridge Scheduler to provide cost-effective, timezone-aware scheduling capabilities' (ScheduleConfiguration doc) — so after the schedule's first firing time passes, ListWorkflowRuns/GetWorkflowRun report a run the caller never started, carrying RunType = SCHEDULED.

Actual: CreateWorkflow (and UpdateWorkflow) accept and persist TriggerMode in the version snapshot and nothing else ever reads it; no code path in the crate creates a run. The only producer of a WorkflowRun record is StartWorkflowRun, which hardcodes run_type = RUN_TYPE_ON_DEMAND (src/ops/start_workflow_run.rs). Consequently the modeled RunType value SCHEDULED is unreachable over the whole HTTP surface: a client that creates a scheduled workflow and polls ListWorkflowRuns sees an empty listing forever, where AWS begins producing scheduled runs. TriggerMode is therefore inert: 'SCHEDULED' and 'ON_DEMAND' produce identical observable behavior, which also means no test or client can tell a misconfigured trigger mode from a working one.

## Evidence

Smithy model: RunType enum = ON_DEMAND | SCHEDULED (smithy_query shape=RunType) with WorkflowRunSummary.RunType/WorkflowRunDetail.RunType reporting it; ScheduleConfiguration.CronExpression doc 'A cron expression that defines when the workflow is automatically executed'; CreateWorkflow/UpdateWorkflow both take TriggerMode and GetWorkflow reports both TriggerMode and ScheduleConfiguration. Handler/state: src/ops/create_workflow.rs lines ~354-364 (trigger_mode cloned into WorkflowSnapshot and never read again except for echoing in GetWorkflow/ListWorkflows summaries), src/ops/start_workflow_run.rs line ~333 (run_type: crate::state::RUN_TYPE_ON_DEMAND — the only place a run is ever created), src/state.rs (no timer, scheduler or cron field anywhere; grep for SCHEDULED in src/ matches only the RunType constant declaration). Test coverage: tests/all/workflow_lifecycle.rs and workflow_update_versions.rs send TriggerMode 'SCHEDULED'/'ON_DEMAND' and assert only that GetWorkflow echoes the value back; no test asserts a scheduled run is ever produced, and no test observes RunType = SCHEDULED. Related but distinct: the open GetWorkflow issue (20261024T002400Z-mwaa-serverless-getworkflow-wrong-behavior) covers ScheduleConfiguration being reported as null; this issue is about no scheduled execution ever happening.

## Steps to Reproduce

1. CreateWorkflow with a valid Name, RoleArn, DefinitionS3Location pointing at a DAG with a short cron schedule, and TriggerMode = 'SCHEDULED'; note the WorkflowArn.
2. Confirm GetWorkflow reports WorkflowStatus = READY and TriggerMode = 'SCHEDULED'.
3. Without calling StartWorkflowRun, wait past the schedule's next firing time and call ListWorkflowRuns for that WorkflowArn.
4. Observe WorkflowRuns = [] indefinitely (and no response anywhere ever carrying RunType = SCHEDULED), where the real service has created one scheduled run per firing and reports it with RunType = SCHEDULED.
