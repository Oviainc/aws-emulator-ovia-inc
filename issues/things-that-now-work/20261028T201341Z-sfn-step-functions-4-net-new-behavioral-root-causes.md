Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.deep-sfn-1-sfn-sync-express-machine-sync-task-rejected-at-runtime stepfunctions.deep-sfn-1-sfn-sync-start-unpermitted-child-rejected stepfunctions.input-output-processing-stages-apply-in-the-documented-order stepfunctions.nested-fire-and-forget-start-execution-does-not-block-on-child
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.deep-sfn-1-sfn-sync-express-machine-sync-task-rejected-at-runtime|run#18|AssertionError|assert _EXPRESS_SYNC_CAUSE in (resp.get("cause") or "") and \
Issue-Key: stepfunctions.deep-sfn-1-sfn-sync-start-unpermitted-child-rejected|run#16|AssertionError|assert bad["status"] == "FAILED", (
Issue-Key: stepfunctions.input-output-processing-stages-apply-in-the-documented-order|run#24|KeyError|state_output = json.loads(exited[0]["stateExitedEventDetails"]["output"])
Issue-Key: stepfunctions.nested-fire-and-forget-start-execution-does-not-block-on-child|run#9|AssertionError|assert "SdkHttpMetadata" in output and "SdkResponseMetadata" in output, (
Issue-Commit: c6f8380220d2b80f1162c28669d8bc5048f04f8c

# Step Functions: 4 net-new behavioral root causes — .sync task on an EXPRESS machine reports emulator-local wording, an unpermitted states:startExecution.sync child is started and the parent SUCCEEDS, StateExited history events carry no output, and the fire-and-forget states:startExecution task result omits the SDK response envelope

> _Note: this issue was identified against the code base at commit `c6f8380220d2b80f1162c28669d8bc5048f04f8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
This issue bundles FOUR distinct net-new root causes in the Step Functions execution interpreter and its execution-history serialization. All four must be closed before submitting; re-replay every check id in trace_ids.

Cause 1 of 4: the Cause text for a '.sync' service integration used inside an EXPRESS state machine is emulator-local wording instead of the AWS refusal.
AWS: an EXPRESS state machine accepts a definition containing a run-a-job ('.sync') Resource at CreateStateMachine, but every execution fails at that state with error 'States.Runtime' and a Cause whose fixed text is "Express state machine does not support '.sync' service integration", with the offending state's name quoted in the Cause (here the variable part is the state name, 'CallChild'); no child job is started. Smithy: StartSyncExecution returns StartSyncExecutionOutput with status/error/cause (error is SensitiveError max 256, cause is SensitiveCause max 32768); the refusal is reported as a FAILED result, not an API error. Recorded behavior supplies the Cause text.
Emulator: status FAILED and error 'States.Runtime' are correct, but the Cause is "a synchronous execution cannot wait for the job a run-a-job integration starts, so the job this definition's task started was not waited for" — it neither contains AWS's fixed sentence nor names the state, and its own wording asserts the task DID start the downstream job, which AWS does not do for this refusal.
Why: a client (or a Catch/Cause-matching wrapper, or an operator reading the failure) that matches on AWS's documented sentence never recognizes the workflow-type restriction, and cannot tell which state caused it because the state name is absent.
Fix: the '.sync' integration dispatch path of the sfn execution interpreter — the EXPRESS workflow-type guard must emit AWS's Cause (fixed sentence + the quoted state name taken from the definition) and must not start the downstream job.
Traces: sfn.deep-sfn-1-sfn-sync-express-machine-sync-task-rejected-at-runtime.

Cause 2 of 4: a 'states:startExecution.sync:2' Task is never authorized against the execution role, so a child the role may not start is started anyway and the parent SUCCEEDS.
AWS: the Task's StartExecution call is made with the state machine's execution role; when that role's policy does not allow states:StartExecution on the target child state machine, the task fails and the execution ends FAILED. The history records exactly one TaskFailed whose taskFailedEventDetails.error is 'StepFunctions.AWSStepFunctionsException' and whose cause embeds the underlying service refusal — fixed fragments 'Error Code: AccessDeniedException' and 'on resource: <child state machine ARN> ', with the ARN taken from the request's Parameters.StateMachineArn — and no TaskSubmitted event is emitted; the child never runs. Smithy: TaskFailedEventDetails carries required resourceType/resource plus error (SensitiveError) and cause (SensitiveCause); StartExecution's modeled errors do not include a success path for an unauthorized caller. Positive control in the same check proves the role is effective for the permitted child.
Emulator: the parent execution ends 'SUCCEEDED' — no authorization is evaluated on the data-plane StartExecution the Task issues, so no TaskFailed event, no AccessDeniedException cause, and the unpermitted child is started.
Why: a least-privilege deployment appears to work on the emulator and breaks in production; worse, a child workflow the role was explicitly denied is actually executed, so a deny is silently converted into an allow.
Fix: authorization evaluation in the sfn interpreter's task-integration dispatch for states:startExecution / startExecution.sync (the execution role's policy must gate the child StartExecution), and the TaskFailed classification that wraps a downstream service refusal as StepFunctions.AWSStepFunctionsException with the service error code and resource in the cause. Related open issue 20261024T043150Z-sfn-step-functions-control-plane-no-iam-passrole-authorization.md covers CONTROL-plane PassRole authorization only and does not carry this data-plane task authorization or this check's replay key.
Traces: sfn.deep-sfn-1-sfn-sync-start-unpermitted-child-rejected.

Cause 3 of 4: StateExited history events are serialized without 'output' (and without 'outputDetails').
AWS: Smithy: StateExitedEventDetails has required 'name' plus 'output' (SensitiveData, @length 0..262144, @sensitive, "The JSON output data of the state"), 'outputDetails', 'assignedVariables' and 'assignedVariablesDetails'. For a Task state, the StateExited event's output is the state's post-processing result — ResultSelector, then ResultPath, then OutputPath already applied.
Emulator: the StateExited event for the Task state has no 'output' member at all, so reading it raises KeyError: 'output'. The execution-level output and the TaskScheduled/TaskSucceeded details in the same history are correct, so only the StateExited serialization is affected.
Why: any client or tool that reconstructs per-state data flow from GetExecutionHistory (debuggers, the console-style step viewer, log-based assertions) cannot see what a state emitted, and code that reads stateExitedEventDetails['output'] crashes rather than seeing a value.
Fix: the history-event construction/serialization for StateExited in the sfn interpreter — populate output (and outputDetails) with the state's post-processing result exactly as AWS does.
Traces: sfn.input-output-processing-stages-apply-in-the-documented-order.

Cause 4 of 4: the fire-and-forget 'arn:aws:states:::states:startExecution' Task result omits the raw SDK response envelope.
AWS: a non-optimized SDK-style integration returns the literal SDK response to the state, i.e. the StartExecution response members plus the SDK metadata envelope: 'ExecutionArn', 'StartDate' (raw epoch-millis integer), 'SdkHttpMetadata' and 'SdkResponseMetadata'. Unlike '.sync', the result is not reshaped and carries none of the child's eventual Status/Output. Smithy: StartExecutionOutput has required executionArn and startDate only — the Sdk*Metadata keys are the integration's envelope, which recorded behavior shows in the task result.
Emulator: the task result is exactly ['ExecutionArn', 'StartDate'] — the SdkHttpMetadata and SdkResponseMetadata envelope members are missing.
Why: a definition whose ResultSelector/ResultPath or downstream Choice reads $.SdkHttpMetadata.HttpStatusCode (a common pattern for unoptimized integrations) fails on the emulator with a missing-path error while it works on AWS, so the emulator cannot be used to validate such workflows.
Fix: the result shaping for unoptimized (non-'.sync') service integrations in the sfn interpreter's task dispatch — wrap the SDK response in the Sdk*Metadata envelope AWS returns.
Traces: sfn.nested-fire-and-forget-start-execution-does-not-block-on-child.

## Evidence
Cause 1: expected a Cause containing "Express state machine does not support '.sync' service integration" and "'CallChild'"; got status FAILED, error 'States.Runtime', cause "a synchronous execution cannot wait for the job a run-a-job integration starts, so the job this definition's task started was not waited for" (AssertionError: the Cause was "...").
Cause 2: expected the parent execution to end FAILED with one TaskFailed carrying error 'StepFunctions.AWSStepFunctionsException' and a cause containing 'Error Code: AccessDeniedException' and 'on resource: <deniedChildArn> '; got AssertionError: "the parent targeting an unpermitted child ended 'SUCCEEDED', expected FAILED" (the positive control against the permitted child succeeded, proving the role was in effect).
Cause 3: reading the Task state's StateExited event raised KeyError: 'output' (state_output = json.loads(exited[0]['stateExitedEventDetails']['output'])); the expected value was the post-processing result {"count": 1}.
Cause 4: AssertionError: "expected the raw StartExecution SDK response envelope (SdkHttpMetadata, SdkResponseMetadata) — this integration is not reshaped the way .sync is; got keys ['ExecutionArn', 'StartDate']".

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (checks listed in trace_ids).
2. Cause 1: create a STANDARD and an EXPRESS state machine from the same definition whose single Task has Resource 'arn:aws:states:::states:startExecution.sync:2'; call StartSyncExecution on the EXPRESS one and read status/error/cause.
3. Cause 2: create two child state machines, grant the execution role states:StartExecution on child A only, then run a parent whose '.sync:2' Task targets child B; read DescribeExecution.status and GetExecutionHistory for TaskFailed/TaskSubmitted, and ListExecutions on child B.
4. Cause 3: run a Task state carrying InputPath, Parameters, ResultSelector, ResultPath and OutputPath; call GetExecutionHistory and read stateExitedEventDetails for that state.
5. Cause 4: run a parent whose Task Resource is 'arn:aws:states:::states:startExecution' (no '.sync') against a slow child; read the parent's execution output keys.
