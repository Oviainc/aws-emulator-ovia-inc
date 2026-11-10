Issue-Source: trace-sweep-divergence
Issue-Trace: glue-workflow-run-properties-roundtrip
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-workflow-run-properties-roundtrip
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetWorkflow|2xx->2xx|$.Workflow.LastRun.Status
Issue-Commit: 0e1266747f28e871e9d7960a7877f6b2763d6dbf

# Glue workflow run settles terminal Status ERROR, so Workflow.LastRun.Status never reaches COMPLETED

> _Note: this issue was identified against the code base at commit `0e1266747f28e871e9d7960a7877f6b2763d6dbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWorkflow

## Description
One net-new root cause. Everything the fixture does before this step succeeds (the workflow is created, the run is started, its run properties are written and read back), so this is neither a bootstrap gap nor an unimplemented stub — the emulator answers GetWorkflow 200 with a fully shaped Workflow, it just reports the wrong terminal run status.

Cause 1 of 1 — a Glue workflow run that has nothing failing in it settles terminal Status 'ERROR' instead of 'COMPLETED', and that wrong status is surfaced through Workflow.LastRun.Status.
AWS: Smithy models Workflow.LastRun as a WorkflowRun ('the information about the last execution of the workflow'), WorkflowRun.Status as the WorkflowRunStatus enum with values RUNNING | COMPLETED | STOPPING | STOPPED | ERROR, and WorkflowRun.Statistics as WorkflowRunStatistics{TotalActions, SucceededActions, FailedActions, StoppedActions, ErroredActions ('the count of job runs in the ERROR state in the workflow run'), TimeoutActions, RunningActions, WaitingActions}. WorkflowRun.ErrorMessage is documented as describing 'any error that may have occurred in starting the workflow run. Currently the only error message is "Concurrent runs exceeded for workflow: foo."'. So a terminal run's Status is derived from the outcomes of its actions: a run in which no action errored, failed, stopped or timed out is COMPLETED, and ERROR is reserved for a run that actually errored. The recording confirms this: after the run started here reaches a terminal state, GetWorkflow reports Workflow.LastRun.Status 'COMPLETED' (status 200).
Emulator: GetWorkflow answers 200 but reports Workflow.LastRun.Status 'ERROR'. The step polls for the run to settle and 'ERROR' is in its fail-on set, so the poll aborts after 2 attempts instead of converging — the emulator's terminal status is ERROR, not a transient value on the way to COMPLETED. No error cause accompanies it; the run is simply marked ERROR.
Why: Workflow.LastRun.Status is the signal an orchestration client polls to decide whether a workflow finished. A client waiting for COMPLETED never sees it and, because ERROR is terminal, concludes the ETL workflow failed — it then skips or reruns downstream work even though nothing in the run failed, and branching on Statistics.ErroredActions/FailedActions finds no failure that explains the ERROR. Any CLI/SDK wait-and-branch loop over a Glue workflow is therefore unusable against the emulator.
Fix: the Glue workflow-run engine must derive a run's terminal Status from its action outcomes rather than defaulting to ERROR — a run whose actions all succeeded (including a run with no failing action) must settle COMPLETED, with Statistics counters consistent with that status and ErrorMessage left unset — and GetWorkflow must project that settled run into Workflow.LastRun. The component is the workflow-run lifecycle/state behind StartWorkflowRun / GetWorkflowRun / GetWorkflow in services/service-com-amazonaws-glue-awsglue, not the GetWorkflow serializer (the response shape is otherwise correct).
Related open issue that does NOT cover this one (the replay reports no owner for this step): 20261109T085123Z-glue-s3-a-workflow-s-conditional-trigger-never.md is a behavioral finding about a CONDITIONAL trigger never firing after its predicate job succeeds, filed on GetWorkflowRun from the glue__s3 behavioral corpus; this wire divergence is the same wrong terminal ERROR surfacing on GetWorkflow's Workflow.LastRun.Status in the run-properties round-trip fixture, which carries no replay key in that issue.
Traces: glue-workflow-run-properties-roundtrip#task#11.

## Evidence
Fixture glue-workflow-run-properties-roundtrip, step task#11, operation GetWorkflow, verdict mismatch.
Expected (recorded AWS): HTTP 200 with $.Workflow.LastRun.Status == 'COMPLETED'.
Actual (emulator): HTTP 200 with $.Workflow.LastRun.Status == 'ERROR'. Replay detail: "$.Workflow.LastRun.Status: value_mismatch (expected 'COMPLETED', got 'ERROR')" and "$.Workflow.LastRun.Status: poll: 'ERROR' is in fail_on after 2 attempt(s) (expected 'COMPLETED', got 'ERROR')" — i.e. the emulator's value is a terminal ERROR that the poll refuses, not an intermediate state.
Status code and response shape are otherwise correct; the single diverging key is the enum value of the run status (WorkflowRunStatus: RUNNING | COMPLETED | STOPPING | STOPPED | ERROR). The run name/id, timestamps and generated ids in the recording are per-run values and are not part of the requirement; the requirement is that a run with no errored/failed/stopped/timed-out action reports COMPLETED.

## Steps to Reproduce
1. glue CreateWorkflow (with DefaultRunProperties), then StartWorkflowRun for it.
2. PutWorkflowRunProperties / GetWorkflowRunProperties for that run id and confirm the round-trip succeeds.
3. Poll GetWorkflow(Name=<workflow>, IncludeGraph) until Workflow.LastRun reaches a terminal Status.
4. Observe Workflow.LastRun.Status 'ERROR' (terminal) where AWS reports 'COMPLETED'.
Replay fixture glue-workflow-run-properties-roundtrip, step task#11.
