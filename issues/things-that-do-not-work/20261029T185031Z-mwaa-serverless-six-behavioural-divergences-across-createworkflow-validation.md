Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mwaa-serverless.create-workflow-duplicate-name-conflict-rejected mwaa-serverless.create-workflow-invalid-engine-version-rejected mwaa-serverless.create-workflow-untrusted-execution-role-access-denied-rejected mwaa-serverless.start-workflow-run-nonexistent-workflow-version-not-found-rejected mwaa-serverless.stop-workflow-run-transitions-to-stopped mwaa-serverless.workflow-run-failure-records-error-message mwaa-serverless.workflow-run-success-end-to-end
Issue-Shape-Id: com.amazonaws.mwaaserverless#AmazonMWAAServerless
Issue-Key: mwaa-serverless.create-workflow-duplicate-name-conflict-rejected|run#13|AssertionError|assert message == "Workflow with given name already exists", (
Issue-Key: mwaa-serverless.create-workflow-invalid-engine-version-rejected|_error#7|AssertionError|raise AssertionError(f"expected rejection but the call SUCCEEDED and returned {result!r}")
Issue-Key: mwaa-serverless.create-workflow-untrusted-execution-role-access-denied-rejected|_error_code_and_message#11|AssertionError|raise AssertionError(
Issue-Key: mwaa-serverless.start-workflow-run-nonexistent-workflow-version-not-found-rejected|run#17|AssertionError|assert message == "Workflow version not found", f"unexpected ResourceNotFoundException message: {message!r}"
Issue-Key: mwaa-serverless.stop-workflow-run-transitions-to-stopped|run#13|AssertionError|raise AssertionError(
Issue-Key: mwaa-serverless.workflow-run-failure-records-error-message|run#14|AssertionError|assert run_state == "FAILED", f"expected RunState=FAILED for a task that exits non-zero, got {run_state!r}"
Issue-Key: mwaa-serverless.workflow-run-success-end-to-end|run#14|KeyError|task_ids = final["RunDetail"]["TaskInstances"]
Issue-Commit: 92c3b140342b1bc1fab2a232b8cee3f25ea78ff5

# MWAA Serverless: six behavioural divergences across CreateWorkflow validation/refusal wording and the workflow-run execution lifecycle

> _Note: this issue was identified against the code base at commit `92c3b140342b1bc1fab2a232b8cee3f25ea78ff5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkflow, StartWorkflowRun, StopWorkflowRun, GetWorkflowRun

## Description
SIX distinct net-new root causes in the `mwaa-serverless` emulator, all grounded in failing behavioural checks. All six must be closed before this issue is submitted.

Model used throughout: services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/smithy-model/com-amazonaws-mwaaserverless-amazonmwaaserverless.json (sdkId "MWAA Serverless", awsJson1_0).

=== Cause 1 of 6: CreateWorkflow's duplicate-Name ConflictException uses the wrong message text ===
AWS: Smithy: CreateWorkflow declares ConflictException (errorKind client, httpError 409) with required members Message/ResourceId/ResourceType. Recorded behavior: for a second CreateWorkflow under a Name an existing workflow already holds, AWS answers HTTP 409, Code `ConflictException`, Message `Workflow with given name already exists`. That message is entirely FIXED text — it has no variable part, it does not interpolate the requested Name, the ARN or any id.
Emulator: answers Code `ConflictException` with Message `A workflow named <Name> already exists` — wrong fixed text, plus a variable part (the request's Name) that AWS's message does not contain.
Why: a client that branches on the refusal reason by matching the documented message string never matches, so a genuine name collision is misclassified as an unknown conflict; an operator reading logs sees a message no AWS runbook contains.
Fix: the CreateWorkflow handler's duplicate-name refusal branch — the message it formats for the non-DELETING holder case. (The sibling "holder is DELETING" message is a different state and is not what this check exercises.)
Traces: mwaa-serverless.create-workflow-duplicate-name-conflict-rejected. Source: services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/create_workflow.rs lines 326-334.

=== Cause 2 of 6: CreateWorkflow accepts an EngineVersion outside the modelled intEnum ===
AWS: Smithy: CreateWorkflowRequest$EngineVersion targets `EngineVersion`, an intEnum whose ONLY defined value is 1 (member ONE, "Engine version 1 of Amazon Managed Workflows for Apache Airflow Serverless"). CreateWorkflow models ValidationException (client, httpError 400) with required Message and required Reason (ValidationExceptionReason: unknownOperation|cannotParse|fieldValidationFailed|other). Recorded behavior: `EngineVersion=999` is refused with HTTP 400, Code `ValidationException`, Message `Invalid engine version` — fixed text, no variable part — and NO workflow is created (the Name does not appear in ListWorkflows). The identical request with `EngineVersion=1` succeeds and reaches WorkflowStatus READY.
Emulator: returns HTTP 200 and CREATES the workflow (WorkflowStatus READY, a minted WorkflowArn, RevisionId and WorkflowVersion). The handler's `validate_input` walks only Name/ClientToken/RoleArn/Description/TriggerMode-style string members; `engine_version` is copied straight onto the version snapshot with no enum-membership check.
Why: this is overly permissive — a caller that typos or hard-codes a future engine version gets a workflow AWS would never have created, so code that works against the emulator fails at create time against AWS, and the emulator's store holds a workflow in a configuration the service has no engine for.
Fix: the CreateWorkflow handler's input validation — add the intEnum membership check for EngineVersion and refuse with ValidationException(Reason=fieldValidationFailed, a FieldList entry naming `EngineVersion`) before anything is minted. UpdateWorkflow carries the same member and should share the one check.
Traces: mwaa-serverless.create-workflow-invalid-engine-version-rejected. Source: services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/create_workflow.rs (validate_input; snapshot field `engine_version: input.engine_version` at line 388).

=== Cause 3 of 6: StartWorkflowRun never checks that the workflow's execution role can be assumed by the service ===
AWS: Smithy: StartWorkflowRun models ValidationException (client, 400), whose doc states it occurs "when required IAM permissions are missing" and that the service "validates workflow definitions, execution roles, and resource configurations". Recorded behavior: role assumability is checked at RUN time, not create time — CreateWorkflow with a RoleArn whose trust policy does NOT name `airflow-serverless.amazonaws.com` SUCCEEDS and leaves the workflow READY (this is the check's own positive control), and the subsequent StartWorkflowRun is refused with HTTP 400, Code `ValidationException`, Message `Workflow execution role could not be assumed by the airflow-serverless service`. That message is fixed text with no variable part. The rejected call records NO run: ListWorkflowRuns stays empty. After UpdateWorkflow repoints the same WorkflowArn at a properly-trusted role, StartWorkflowRun succeeds and the RunId appears in ListWorkflowRuns.
Emulator: StartWorkflowRun returns HTTP 200 with a RunId and Status STARTING for a workflow whose role trusts only `lambda.amazonaws.com`. The handler resolves the version, the DELETING state and the run quota, and never looks at the snapshot's `role_arn` trust relationship at all.
Why: overly permissive on the data plane — every run against a misconfigured execution role appears to start and then reports SUCCESS, so the single most common real-world MWAA Serverless misconfiguration is undetectable against the emulator and surfaces only in production.
Fix: the StartWorkflowRun handler — before admitting the run, resolve the active version's RoleArn against the emulated IAM role's trust policy and refuse with the modelled ValidationException when `airflow-serverless.amazonaws.com` is not a trusted principal; the refusal must leave the run store untouched. NOTE: open issue 20261024T002400Z-mwaa-serverless-createworkflow-validation.md asks for a non-assumable role to be refused by CreateWorkflow — it does not cover this cause and must not be satisfied by moving the check to create time: this check asserts CreateWorkflow still accepts the untrusted role and leaves the workflow READY.
Traces: mwaa-serverless.create-workflow-untrusted-execution-role-access-denied-rejected. Source: services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/start_workflow_run.rs lines 210-405.

=== Cause 4 of 6: StartWorkflowRun's version-not-found ResourceNotFoundException uses the wrong message text ===
AWS: Smithy: StartWorkflowRun models ResourceNotFoundException (client, httpError 404) with required Message/ResourceId/ResourceType; StartWorkflowRunRequest$WorkflowVersion is "Optional. The specific version of the workflow to execute. If not specified, the latest version is used." Recorded behavior: StartWorkflowRun pinned to a well-formed but never-created WorkflowVersion is refused with HTTP 404, Code `ResourceNotFoundException`, Message `Workflow version not found` — fixed text, no variable part (the requested version id is carried on ResourceId, not interpolated into the message) — and records no new run (ListWorkflowRuns count is unchanged).
Emulator: answers Code `ResourceNotFoundException` with Message `Unable to find the requested version`. The code and the effect-absence are right; only the wording diverges.
Why: a client distinguishing "this workflow is gone" from "this pinned version is gone" on the documented message text cannot, so a retry that should drop the version pin is not attempted.
Fix: the shared unresolved-entity refusal builder the StartWorkflowRun handler reaches for a missing version — the phrasing it emits for the version entity.
Traces: mwaa-serverless.start-workflow-run-nonexistent-workflow-version-not-found-rejected. Source: services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/start_workflow_run.rs lines 244-252 (`crate::state::unresolved` / `UnresolvedEntity::version_for`).

=== Cause 5 of 6: a DAG whose `tasks` collection is nested under the DAG-id key yields a run with ZERO task instances ===
AWS: Smithy: WorkflowRunDetail$TaskInstances is "A list of task instance IDs that are part of this workflow run"; GetTaskInstance/ListTaskInstances report each instance's TaskId, Status, StartedAt and EndedAt. Recorded behavior: for the Airflow YAML DSL shape the suite uploads to DefinitionS3Location —
  <dag_id>:
    start_date: "..."
    schedule: null
    tasks:
      <task_id>:
        operator: airflow.providers.standard.operators.bash.BashOperator
        bash_command: "..."
— AWS materialises ONE task instance per declared task. A two-task DAG gives GetWorkflowRun a RunDetail with TaskInstances of length 2, ListTaskInstances returns exactly those ids, every instance reports Status SUCCESS with non-null StartedAt and EndedAt, and the run carries a non-null CompletedOn.
Emulator: GetWorkflowRun's RunDetail OMITS `TaskInstances` entirely (the check dies with KeyError: 'TaskInstances'), and the run settles to a terminal state having executed nothing. The cause is the definition reader's task-path predicate: it recognises a `tasks` collection ONLY at the document root or at `dag.tasks`, never at `<dag_id>.tasks`, which is the canonical Airflow shape where the DAG id is the top-level mapping key. Zero declarations are found, so the execution plan is empty, so no instance is ever recorded. The same path restriction applies to the schedule walk, which likewise cannot see `<dag_id>.schedule`.
Why: the whole data plane is hollow for the standard definition layout — a client cannot observe task fan-out, per-task status, or task timing, and TaskInstances being absent rather than an empty list breaks SDK callers that index the modelled member. Worse, an empty plan means every run reports SUCCESS without having executed any of the caller's tasks.
Why (second effect, same cause): because the plan is empty the run races straight through STARTING/QUEUED/RUNNING to SUCCESS, so a run of a DAG whose single task is `sleep 240` — which AWS keeps non-terminal for minutes — is already terminal by the time a caller can call StopWorkflowRun. The check fails before StopWorkflowRun is ever issued ("run reached a terminal state before StopWorkflowRun could be issued: SUCCESS"), so StopWorkflowRun's modelled STOPPING/STOPPED transition is unreachable in practice. AWS: StopWorkflowRun returns Status STOPPING or STOPPED and the run settles in RunState STOPPED (WorkflowRunStatus = STARTING|QUEUED|RUNNING|SUCCESS|FAILED|TIMEOUT|STOPPING|STOPPED).
Fix: the definition reader's task (and schedule) key-path predicate in the service state's workflow-definition module — accept the DAG-level collection under an arbitrary single DAG-id mapping key as well as at the root and under `dag`; and the run settlement in the service state must give a run with declared work a non-terminal window long enough for a stop to be issued against it.
Traces: mwaa-serverless.workflow-run-success-end-to-end, mwaa-serverless.stop-workflow-run-transitions-to-stopped. Source: services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/state/workflow_definition.rs lines 693-702 (`at_tasks_path`) and 503-511 (`at_schedule_path`); services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/state/workflow_execution.rs lines 93-111.

=== Cause 6 of 6: a run whose task fails is still reported SUCCESS, with no ErrorMessage ===
AWS: Smithy: WorkflowRunDetail$RunState is a WorkflowRunStatus (STARTING|QUEUED|RUNNING|SUCCESS|FAILED|TIMEOUT|STOPPING|STOPPED) and WorkflowRunDetail$ErrorMessage is "The error message if the workflow run failed. This value is null if the run completed successfully" (GenericString, length 1..255). Recorded behavior: a BashOperator task whose `bash_command` is `exit 1` fails deterministically; Airflow retries it, so the run can carry more than one task instance for that task distinguished by AttemptNumber, but NO attempt ever reports Status SUCCESS, the final attempt reports Status FAILED, the run's RunDetail.RunState is `FAILED`, and RunDetail.ErrorMessage is non-empty.
Emulator: the run reports RunState `SUCCESS` and sets no ErrorMessage. The execution planner states outright that it "invents no failure for a task": the only FAILED outcome it can produce is a definition whose graph could not be ORDERED (unknown dependency, cycle, duplicate or unreportable task id, too many tasks). A task that runs and exits non-zero has no representation at all, so a deterministically-failing DAG is indistinguishable from a succeeding one.
Why: the failure path of the data plane cannot be exercised — retry/alerting logic keyed on RunState==FAILED or on a non-null ErrorMessage never fires against the emulator, so a client's error handling is never tested and ships untried. Combined with Cause 5 it means every run of every definition reports SUCCESS.
Fix: the run settlement in the service state, driven from the execution plan — derive each task instance's terminal Status from the task declaration the definition carries (a command that cannot succeed must yield a FAILED instance), propagate a failed instance to the run's RunState FAILED, and record a RunDetail.ErrorMessage naming the failing task within the modelled 1..255 length.
Traces: mwaa-serverless.workflow-run-failure-records-error-message. Source: services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/state/workflow_execution.rs lines 40-46 and 87-135.

## Evidence
Suite: simparity-behavioral-aws-v1. mwaa-serverless 15/22 (68%), 7 failed (6 mismatch, 1 error). No failure was marked ALREADY FILED or OWNED by an open issue.

Cause 1 — create-workflow-duplicate-name-conflict-rejected:
  expected ConflictException Message == "Workflow with given name already exists"
  actual   AssertionError: unexpected ConflictException message: 'A workflow named sim2real-mwaas-dupname-63fe00a02150 already exists'
  (the recorded Name `sim2real-mwaas-dupname-63fe00a02150` is a per-run generated value, not part of the requirement; AWS's message interpolates nothing)

Cause 2 — create-workflow-invalid-engine-version-rejected:
  expected HTTP 400 ValidationException, Message "Invalid engine version", and the Name absent from ListWorkflows
  actual   AssertionError: expected rejection but the call SUCCEEDED and returned
           {'WorkflowArn': 'arn:aws:airflow-serverless:us-west-2:000000000000:workflow/sim2real-mwaas-badengine-...', 'WorkflowStatus': 'READY', 'IsLatestVersion': True, ... 'HTTPStatusCode': 200}
  (ARN, RevisionId, WorkflowVersion and timestamps above are per-run values; the divergence is the 200 + created workflow)

Cause 3 — create-workflow-untrusted-execution-role-access-denied-rejected:
  control passed: CreateWorkflow with a RoleArn trusting only lambda.amazonaws.com returned 200 and GetWorkflow reported WorkflowStatus READY (matching AWS)
  expected StartWorkflowRun -> HTTP 400 ValidationException, Message "Workflow execution role could not be assumed by the airflow-serverless service", ListWorkflowRuns == []
  actual   AssertionError: expected the call to be refused for an untrusted execution role, but it SUCCEEDED:
           {'RunId': 'run-fea334d9be461f79', 'Status': 'STARTING', ... 'HTTPStatusCode': 200}

Cause 4 — start-workflow-run-nonexistent-workflow-version-not-found-rejected:
  Code matched (ResourceNotFoundException) and no run was recorded; only the message diverged.
  expected Message == "Workflow version not found"
  actual   AssertionError: unexpected ResourceNotFoundException message: 'Unable to find the requested version'

Cause 5 — workflow-run-success-end-to-end (verdict: error) and stop-workflow-run-transitions-to-stopped:
  success-end-to-end: KeyError: 'TaskInstances' at `task_ids = final["RunDetail"]["TaskInstances"]` — GetWorkflowRun's RunDetail has no TaskInstances member at all for a run of a two-task DAG (expected a list of length 2, with ListTaskInstances returning the same ids and every instance Status SUCCESS with non-null StartedAt/EndedAt).
  stop-workflow-run: AssertionError: run reached a terminal state before StopWorkflowRun could be issued: SUCCESS — the DAG's single task is `sleep 240`, which AWS keeps non-terminal for minutes; expected StopWorkflowRun to answer Status in (STOPPING, STOPPED) and the run to settle in RunState STOPPED.
  Code evidence for the shared cause: src/state/workflow_definition.rs `at_tasks_path` matches only `Some([key]) if key == "tasks"` and `Some(["dag", key]) if key == "tasks"`; the uploaded definition nests `tasks` under the DAG id (`sim2real_dag.tasks`), so `declared_tasks` returns none and `ExecutionPlan` is empty.

Cause 6 — workflow-run-failure-records-error-message:
  DAG uploaded: single BashOperator task `only_task` with bash_command "exit 1".
  expected RunDetail.RunState == "FAILED", RunDetail.ErrorMessage non-empty, >=1 task instance all belonging to only_task, no attempt reporting SUCCESS, final attempt Status FAILED
  actual   AssertionError: expected RunState=FAILED for a task that exits non-zero, got 'SUCCESS'
  Code evidence: src/state/workflow_execution.rs — "Nothing here invents a failure for a task"; `build_error` (the only FAILED path) is set solely for an unorderable graph.

## Steps to Reproduce
Common setup for every cause (as each check does it, self-contained): create an S3 bucket, PutObject an Airflow YAML DAG at `dags/<name>.yaml`, and create an IAM role whose trust policy names `airflow-serverless.amazonaws.com` (except where stated).

Cause 1: CreateWorkflow(Name=N, DefinitionS3Location={Bucket,ObjectKey}, RoleArn=R). Call CreateWorkflow again with the SAME Name and inspect Error.Code/Error.Message. Expect 409 ConflictException / "Workflow with given name already exists". Then GetWorkflow on the first ARN and confirm WorkflowVersion is unchanged, and that a differently-named create still reaches READY.

Cause 2: CreateWorkflow(Name=N, DefinitionS3Location=..., RoleArn=R, EngineVersion=999). Expect 400 ValidationException / "Invalid engine version"; then ListWorkflows and confirm N is absent. Repeat with EngineVersion=1 and confirm GetWorkflow reports WorkflowStatus READY.

Cause 3: create an IAM role whose trust policy names `lambda.amazonaws.com` (NOT airflow-serverless). CreateWorkflow with that RoleArn — expect 200 and GetWorkflow WorkflowStatus READY. Then StartWorkflowRun(WorkflowArn) and expect 400 ValidationException / "Workflow execution role could not be assumed by the airflow-serverless service", with ListWorkflowRuns still empty. Then UpdateWorkflow to a properly-trusted RoleArn and confirm StartWorkflowRun returns a RunId that appears in ListWorkflowRuns.

Cause 4: CreateWorkflow, then StartWorkflowRun(WorkflowArn) once without a version pin (control, must return a RunId). Then StartWorkflowRun(WorkflowArn, WorkflowVersion=<32-hex-char value that was never created>) and expect 404 ResourceNotFoundException / "Workflow version not found", with the ListWorkflowRuns count unchanged.

Cause 5: upload a DAG of the form `<dag_id>:` / `  tasks:` / `    first_task: {operator: ..., bash_command: "echo a"}` / `    second_task: {...}`. CreateWorkflow, StartWorkflowRun, poll GetWorkflowRun to a terminal RunState, then read RunDetail.TaskInstances — expect a list of 2 ids; ListTaskInstances must return the same ids and each GetTaskInstance must report Status SUCCESS with non-null StartedAt/EndedAt and a non-null RunDetail.CompletedOn. Second effect: upload a DAG whose single task has bash_command "sleep 240", StartWorkflowRun, poll GetWorkflowRun until RunState is non-terminal, call StopWorkflowRun(WorkflowArn, RunId) — expect Status STOPPING or STOPPED and the run to settle in RunState STOPPED.

Cause 6: upload a DAG whose single task `only_task` has bash_command "exit 1". CreateWorkflow, StartWorkflowRun, poll GetWorkflowRun to a terminal state. Expect RunDetail.RunState == FAILED, RunDetail.ErrorMessage non-empty, at least one task instance, every instance's TaskId == "only_task", no instance reporting Status SUCCESS, and the highest-AttemptNumber instance reporting Status FAILED.
