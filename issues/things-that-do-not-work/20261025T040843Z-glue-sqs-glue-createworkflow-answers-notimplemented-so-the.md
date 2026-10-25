Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__sqs.glue-workflow-state-change-to-sqs-positive
Issue-Key: glue__sqs|CreateWorkflow|NotImplemented
Issue-Commit: 2638cac857b16c670648ae4436fc785f6120a46b

# glue__sqs: Glue CreateWorkflow answers NotImplemented, so the workflow-driven Glue Job State Change event never reaches SQS and GetWorkflowRun can never be cross-checked

> _Note: this issue was identified against the code base at commit `2638cac857b16c670648ae4436fc785f6120a46b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkflow

## Description
Cause 1 of 1: glue CreateWorkflow is an unimplemented scaffold stub, which refuses the first workflow call of the glue+sqs event-propagation check and leaves the whole workflow -> EventBridge -> SQS path ungraded.

AWS: Glue CreateWorkflow returns HTTP 200 with CreateWorkflowResponse carrying the created workflow's Name (Smithy: glue CreateWorkflow, required input Name (NameString, @length 1..255), optional Description/DefaultRunProperties/MaxConcurrentRuns/Tags (@length 0..50); modeled errors AlreadyExistsException 400, InvalidInputException 400, ConcurrentModificationException 400, ResourceNumberLimitExceededException 400, OperationTimeoutException 400, InternalServiceException 500). NotImplemented is not a modeled error of this operation, and no 4xx/5xx is correct for a valid, non-conflicting Name. Recorded behavior for the rest of the path: an ON_DEMAND trigger bound to the workflow fires the named job, the job run publishes an EventBridge event with source "aws.glue" and detail-type "Glue Job State Change" whose detail.state reaches SUCCEEDED, that event is delivered to the SQS target of a matching rule, and glue GetWorkflowRuns/GetWorkflowRun(IncludeGraph=true) report a run that reaches Status COMPLETED whose Graph JOB node lists the same job run id under JobDetails.JobRuns (Smithy: glue StartWorkflowRun -> RunId; GetWorkflowRun(Name, RunId, IncludeGraph) -> Run, errors EntityNotFoundException 400).

Emulator: CreateWorkflow is refused with error code NotImplemented and the message "operation CreateWorkflow is scaffolded but not implemented in this emulator", so the check errors at setup and nothing downstream (trigger start, event emission, SQS delivery, workflow run graph) is exercised. Related open issues exist for the same operation reached from other check groups - the auto-filed glue CreateWorkflow stub issue naming services/service-com-amazonaws-glue-awsglue/src/ops/create_workflow.rs (grounded in glue.workflow-run-lifecycle) and 20261025T002357Z-events-glue-glue-createworkflow-is-an-unimplemented-scaffold-stub-so.md (grounded in the events__glue batch-size check) - but neither carries this cross-service check's replay key, and this check additionally grades event propagation into SQS and the run-graph cross-check, which those do not.

Why: a client that orchestrates Glue work through workflows cannot create one at all; every workflow-based pipeline fails at its first control-plane call. Because the operation is refused rather than wrong, downstream contracts stay unverified: consumers that react to "Glue Job State Change" events delivered to a queue, and tooling that correlates a delivered detail.jobRunId back to the workflow run's graph, have nothing to run against.

Fix: the CreateWorkflow handler, services/service-com-amazonaws-glue-awsglue/src/ops/create_workflow.rs, plus the workflow state it needs - workflow registry with Tags/MaxConcurrentRuns, trigger association (CreateTrigger with WorkflowName, StartTrigger for ON_DEMAND), workflow run records readable through GetWorkflowRuns/GetWorkflowRun with an IncludeGraph JOB node whose JobDetails.JobRuns carry the job run ids, and emission of the "aws.glue" / "Glue Job State Change" event for a workflow-driven job run so an EventBridge rule can route it to an SQS target.

Traces: glue__sqs.glue-workflow-state-change-to-sqs-positive

## Evidence
Expected (real AWS): CreateWorkflow(Name=<unique>, Tags={...}) -> 200 {"Name": "<unique>"}.

Actual (emulator):
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateWorkflow operation: operation CreateWorkflow is scaffolded but not implemented in this emulator

The check is reported as [error] glue__sqs.glue-workflow-state-change-to-sqs-positive and is NOT marked ALREADY FILED or OWNED in the replay report, so no filed issue carries its replay key. Everything after the CreateWorkflow call is unreached: the ON_DEMAND trigger start, the SQS delivery of the detail-type "Glue Job State Change" event with detail.state == "SUCCEEDED", the workflow run reaching Status COMPLETED, and the assertion that the delivered detail.jobRunId appears among the workflow run graph's JOB node JobDetails.JobRuns.

## Steps to Reproduce
1. s3 CreateBucket and PutObject a trivial pythonshell script; iam CreateRole for a Glue job role; glue CreateJob for a pythonshell job pointing at that script.
2. glue CreateWorkflow(Name="sim2real-glue-wf-<unique>", Tags={"sim2real-check": "glue-workflow-state-change-to-sqs-positive"}) -> emulator answers NotImplemented; AWS answers 200 with the workflow Name.
3. (unreached) glue CreateTrigger(Name=..., WorkflowName=<workflow>, Type="ON_DEMAND", Actions=[{"JobName": <job>}]).
4. (unreached) events PutRule with pattern {"source":["aws.glue"],"detail-type":["Glue Job State Change"],"detail":{"jobName":[<job>],"state":["SUCCEEDED"]}}, sqs CreateQueue, set the queue policy for the rule, events PutTargets with the queue ARN.
5. (unreached) glue StartTrigger, then receive from the queue until a body whose detail.jobName is the job; assert detail.state == "SUCCEEDED".
6. (unreached) glue GetWorkflowRuns / GetWorkflowRun(IncludeGraph=True) and assert the delivered detail.jobRunId is among the JOB node's JobDetails.JobRuns and the run Status is COMPLETED.
