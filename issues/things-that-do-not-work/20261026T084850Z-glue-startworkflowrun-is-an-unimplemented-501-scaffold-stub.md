Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.workflow-run-lifecycle
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.workflow-run-lifecycle|run#9|AssertionError|assert code == "InvalidInputException", (
Issue-Commit: 13273ef63a283393f8b97bbdcadeb04fa836ecfd

# Glue StartWorkflowRun is an unimplemented 501 scaffold stub, so a bare workflow is never refused with InvalidInputException and no workflow run can ever be started

> _Note: this issue was identified against the code base at commit `13273ef63a283393f8b97bbdcadeb04fa836ecfd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartWorkflowRun

## Description
ONE net-new root cause.

Cause 1 of 1: `glue:StartWorkflowRun` is still the generated scaffold stub, so every start of a Glue workflow run — the refusal case and the success case alike — is answered with 501 `NotImplemented` instead of the modeled behavior. Failing check ids: glue.workflow-run-lifecycle.

AWS: Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json) models `StartWorkflowRun` with input `Name` (NameString, @required, @length 1..255) and optional `RunProperties` (WorkflowRunProperties map), output `RunId` (IdString, @length 1..255), and errors `ConcurrentRunsExceededException`, `EntityNotFoundException`, `InvalidInputException`, `OperationTimeoutException`, `ResourceNumberLimitExceededException` (all errorKind=client, HTTP 400) and `InternalServiceException` (HTTP 500). The modeled doc is "Starts a new run of the specified workflow." A Glue workflow is only runnable once a *starting* trigger exists — an ON_DEMAND trigger bound to that workflow via `CreateTrigger(WorkflowName=...)`. Starting a workflow that has no such entry point is refused with HTTP 400, error code `InvalidInputException`, and the message `Workflow <name> doesn't have any starting trigger.` — the fixed text is `Workflow ` ... ` doesn't have any starting trigger.` and the only variable part is the workflow name taken verbatim from the request's `Name` member (the recording's `sim2real-behav-wf-<suffix>` value is one instance filling that template, not part of it). Once a starting trigger exists, the same call returns HTTP 200 with a fresh `RunId`, and that run is readable through `GetWorkflowRun`/`GetWorkflowRuns` (WorkflowRunId echoing the returned RunId, `Name` echoing the workflow) and advances to a terminal `WorkflowRunStatus` of COMPLETED, STOPPED or ERROR.

Emulator: every `StartWorkflowRun` request is refused with HTTP 501, error code `NotImplemented`, message `operation StartWorkflowRun is scaffolded but not implemented in this emulator`. The handler body is the untouched scaffold: `Err(crate::scaffolded::Scaffolded::not_implemented("StartWorkflowRun"))`, and the file still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker. The replay shows this on the negative leg — the check asserted `InvalidInputException` for a workflow with no starting trigger and got `NotImplemented` — and the same stub blocks the positive leg (start a run, poll it to a terminal status, read it back through GetWorkflowRun/GetWorkflowRuns) in the same check.

Why: a client cannot start a Glue workflow at all. The no-starting-trigger precondition — the thing that tells an orchestration author their workflow has no entry point — is never signalled, and because 501/`NotImplemented` is not among the operation's modeled errors, SDK retry and error-handling code branching on `InvalidInputException`, `EntityNotFoundException` or `ConcurrentRunsExceededException` takes none of its branches and surfaces an unmodeled failure. The whole workflow-run lifecycle (RunId minting, status progression, GetWorkflowRun/GetWorkflowRuns readback) is unreachable downstream of this one refusal.

Fix: belongs in the operation's handler and the service's workflow state — `services/service-com-amazonaws-glue-awsglue/src/ops/start_workflow_run.rs` (the `handle` body), which must resolve the workflow and its bound triggers out of `crate::state::service_state`, refuse a workflow with no ON_DEMAND starting trigger with the modeled `InvalidInputException` and AWS's message template, and otherwise mint and record a workflow run that sibling readers (`get_workflow_run.rs`, `get_workflow_runs.rs`) can observe advancing to a terminal status. The SCAFFOLDED marker line must be deleted once implemented.

Related but NOT this cause: `glue CreateTrigger` is a separate open stub already filed as 20261025T022820Z-glue-createtrigger-stub.md, and the later legs of this same check (graph nodes, run-to-terminal, BatchGetWorkflows MissingWorkflows) cannot be reached until that one lands too. This issue covers only StartWorkflowRun's own refusal.

Traces: glue.workflow-run-lifecycle (verdict: mismatch — "start_workflow_run on a workflow with no starting trigger must be rejected, got NotImplemented").

## Evidence
Expected (real AWS, per Smithy model + the check's recorded contract):
  * StartWorkflowRun on a workflow with no starting trigger -> HTTP 400, Error.Code = "InvalidInputException", Error.Message = "Workflow <Name> doesn't have any starting trigger." (fixed text; <Name> is the request's Name member).
  * StartWorkflowRun on a workflow with an ON_DEMAND trigger bound to it -> HTTP 200, body { "RunId": "<id>" }, and GetWorkflowRun(Name, RunId) reports Run.WorkflowRunId == RunId, Run.Name == Name, and Run.Status reaching one of COMPLETED | STOPPED | ERROR.

Actual (emulator, replay of glue.workflow-run-lifecycle):
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartWorkflowRun operation: operation StartWorkflowRun is scaffolded but not implemented in this emulator
  HTTP status 501, Error.Code = "NotImplemented" (not a modeled error of this operation).
  Check assertion that fired: `assert code == "InvalidInputException"` ->
  "AssertionError: start_workflow_run on a workflow with no starting trigger must be rejected, got NotImplemented"

Source evidence in the checkout:
  services/service-com-amazonaws-glue-awsglue/src/ops/start_workflow_run.rs line 4: `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented`
  services/service-com-amazonaws-glue-awsglue/src/ops/start_workflow_run.rs line 167: `Err(crate::scaffolded::Scaffolded::not_implemented("StartWorkflowRun"))`
  Modeled errors available but unused (lines 95-102): ConcurrentRunsExceededException, EntityNotFoundException, InternalServiceException, InvalidInputException, OperationTimeoutException, ResourceNumberLimitExceededException.

## Steps to Reproduce
1. Boot the emulator and point a botocore/AWS SDK glue client at it.
2. glue.create_workflow(Name=<wf>, Description="behavioral test workflow") — succeeds.
3. Create a Glue-assumable IAM role and glue.create_job(Name=<job>, Role=<role-arn>, Command={"Name":"pythonshell","PythonVersion":"3.9","ScriptLocation":"s3://<bucket>/scripts/etl.py"}, MaxCapacity=0.0625, Timeout=10).
4. Call glue.start_workflow_run(Name=<wf>) on the bare workflow (no trigger bound yet).
   EXPECTED: ClientError with HTTP 400, Code "InvalidInputException", Message "Workflow <wf> doesn't have any starting trigger."
   ACTUAL:   ClientError with HTTP 501, Code "NotImplemented", Message "operation StartWorkflowRun is scaffolded but not implemented in this emulator".
5. (Blocked downstream, same operation) bind an ON_DEMAND trigger with glue.create_trigger(Name=<trig>, Type="ON_DEMAND", WorkflowName=<wf>, Actions=[{"JobName":<job>}]) and call glue.start_workflow_run(Name=<wf>) again.
   EXPECTED: HTTP 200 with a RunId, then glue.get_workflow_run(Name=<wf>, RunId=<id>) reaching Run.Status in {COMPLETED, STOPPED, ERROR}.
   ACTUAL:   the same 501 NotImplemented.

Replay shortcut: re-run behavioral check `glue.workflow-run-lifecycle`.
