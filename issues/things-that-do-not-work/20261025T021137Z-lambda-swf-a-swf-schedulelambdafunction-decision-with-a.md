Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__swf.schedule-lambda-function-without-role-rejected
Issue-Key: lambda__swf.schedule-lambda-function-without-role-rejected|_poll_until#35|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: b53a5949fc2b39619101df0f3be30e501a90ea3a

# lambda__swf: a SWF ScheduleLambdaFunction decision with a valid lambdaRole is never invoked — no LambdaFunctionStarted/Completed/Failed is ever recorded (1 cause)

> _Note: this issue was identified against the code base at commit `b53a5949fc2b39619101df0f3be30e501a90ea3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RespondDecisionTaskCompleted

## Description
One net-new root cause.

Cause 1 of 1: a SCHEDULE_LAMBDA_FUNCTION decision on a workflow execution that DOES carry a lambdaRole is accepted and recorded as LambdaFunctionScheduled, but the Lambda task then never progresses: the emulator records no LambdaFunctionStarted, no LambdaFunctionCompleted, no LambdaFunctionFailed, no LambdaFunctionTimedOut and no StartLambdaFunctionFailed. The Lambda task hangs in the Scheduled state forever.
Failing check id: lambda__swf.schedule-lambda-function-without-role-rejected.

AWS: Smithy (com.amazonaws.swf) models the full Lambda-task event sequence in EventType: LambdaFunctionScheduled, LambdaFunctionStarted, LambdaFunctionCompleted, LambdaFunctionFailed, LambdaFunctionTimedOut, plus the two failure-to-dispatch events ScheduleLambdaFunctionFailed and StartLambdaFunctionFailed (whose only modelled cause, StartLambdaFunctionFailedCause, is ASSUME_ROLE_FAILED). LambdaFunctionStartedEventAttributes requires scheduledEventId; LambdaFunctionCompletedEventAttributes requires scheduledEventId and startedEventId and carries the function's `result` (Data, @length 0..32768). On real AWS a ScheduleLambdaFunction decision therefore always reaches a terminal Lambda event: either SWF assumes the execution's lambdaRole and invokes the function — recording LambdaFunctionStarted and then LambdaFunctionCompleted/Failed/TimedOut — or dispatch fails and SWF records StartLambdaFunctionFailed (cause ASSUME_ROLE_FAILED) or ScheduleLambdaFunctionFailed. A Lambda task never remains indefinitely in LambdaFunctionScheduled.

Emulator: the negative half of the check is honoured — for an execution started with NO lambdaRole, the emulator records LambdaFunctionScheduled and then StartLambdaFunctionFailed with cause ASSUME_ROLE_FAILED, and emits no LambdaFunctionStarted/Completed. But the positive control, which starts the SAME workflow type against the SAME domain and the SAME Lambda function with a correctly-trusted lambdaRole (sts:AssumeRole for swf.amazonaws.com, inline lambda:InvokeFunction on the function ARN), diverges. RespondDecisionTaskCompleted returns 200 and GetWorkflowExecutionHistory shows exactly WorkflowExecutionStarted, DecisionTaskScheduled, DecisionTaskStarted, DecisionTaskCompleted, LambdaFunctionScheduled — and then nothing further for the 30 s poll window. The check fails at its control poll with `AssertionError: control attempt 1 to settle not met within 30s`, because no new event whose type is in {LambdaFunctionCompleted, LambdaFunctionFailed, LambdaFunctionTimedOut, StartLambdaFunctionFailed, ScheduleLambdaFunctionFailed} ever appears. The emulator thus implements only the assume-role-failure branch of the Lambda-task path and has no success branch: the referenced Lambda function is never actually invoked, so the result the decider waits on (here {"sum": 6}) is never produced.

Why: a client gets a workflow that silently wedges. Any decider that schedules a Lambda task — the standard SWF alternative to an activity worker — blocks forever: the follow-up decision task that AWS schedules on the terminal Lambda event is never created, so the workflow makes no further progress and eventually only times out at its executionStartToCloseTimeout instead of completing. The asymmetry is worse than a flat omission: because the no-role case DOES produce ASSUME_ROLE_FAILED, a client testing role wiring cannot distinguish "the role is wrong" from "a correct role also produces nothing", and a test suite that asserts the negative passes while the positive path it is calibrated against is entirely absent.

Fix: SWF's decision-processing path for the ScheduleLambdaFunction decision type — the RespondDecisionTaskCompleted handler and the workflow-execution state machine that owns history append — must, after recording LambdaFunctionScheduled, actually dispatch the named function through Lambda Invoke under the execution's lambdaRole and then record LambdaFunctionStarted followed by LambdaFunctionCompleted (with scheduledEventId, startedEventId and the invocation result), or LambdaFunctionFailed / LambdaFunctionTimedOut (honouring the decision's startToCloseTimeout) on an invocation error or timeout, and in every case schedule the follow-up decision task. Note that Lambda Invoke not executing deployment packages is already filed separately (20261024T055421Z-lambda-opensearch-s3-lambda-invoke-cannot-execute-a-deployment-package-so.md) and may be a prerequisite; the divergence reported here is in the SWF Lambda-task state machine, which emits no terminal Lambda event at all and so does not even reach an invoke.

Traces: lambda__swf.schedule-lambda-function-without-role-rejected.

## Evidence
Failing check: lambda__swf.schedule-lambda-function-without-role-rejected (verdict: mismatch).

Expected (real AWS): after RespondDecisionTaskCompleted carrying a ScheduleLambdaFunction decision on an execution started with a valid lambdaRole, GetWorkflowExecutionHistory eventually contains LambdaFunctionScheduled, then LambdaFunctionStarted, then a terminal Lambda event — normally LambdaFunctionCompleted whose lambdaFunctionCompletedEventAttributes.result is the function's return value (the check's handler returns {"sum": 6} for input {"numbers": [1,2,3]}). StartLambdaFunctionFailed/ScheduleLambdaFunctionFailed are the only alternatives; one of the set is always reached.

Actual (emulator): the control execution's history stops at LambdaFunctionScheduled and never gains another event. Replay output:

  File "check_lambda__swf_schedule_lambda_function_without_role_rejected.py", line 173, in run
    control_events = _poll_until(lambda: swf.get_workflow_execution_history(...))
  AssertionError: control attempt 1 to settle not met within 30s. Last result:
    [{'eventType': 'WorkflowExecutionStarted', 'eventId': 1, 'workflowExecutionStartedEventAttributes': {'executionStartToCloseTimeout': '600', 'taskStartToCloseTimeout': '60', 'childPolicy': 'TERMINATE', 'taskList': {...}, 'workflowType': {'name': 'sim2real-swflam-norole-wf-type', 'version': '1.0'}, 'parentInitiatedEventId': 0, 'lambdaRole': 'arn:aws:iam::...:role/sim2real/...-role-swf-...'}},
     {'eventType': 'DecisionTaskScheduled', 'eventId': 2, ...},
     {'eventType': 'DecisionTaskStarted', 'eventId': 3, ...},
     {'eventType': 'DecisionTaskCompleted', 'eventId': 4, ...},
     {'eventType': 'LambdaFunctionScheduled', ...}]

The predicate being waited on is `any(e['eventId'] not in before_ids and e['eventType'] in {LambdaFunctionCompleted, LambdaFunctionFailed, LambdaFunctionTimedOut, StartLambdaFunctionFailed, ScheduleLambdaFunctionFailed})` — i.e. ANY terminal Lambda-task outcome, success or failure. None occurs. Note the started event DOES carry the lambdaRole, so the execution is correctly configured; the emulator simply never dispatches.

By contrast the negative half of the same check passes against the emulator: the no-lambdaRole execution does record LambdaFunctionScheduled followed by StartLambdaFunctionFailed with cause 'ASSUME_ROLE_FAILED' and no LambdaFunctionStarted — so only the success branch of the Lambda-task state machine is missing.

Model citations: com.amazonaws.swf#EventType includes LambdaFunctionScheduled, LambdaFunctionStarted, LambdaFunctionCompleted, LambdaFunctionFailed, LambdaFunctionTimedOut, ScheduleLambdaFunctionFailed, StartLambdaFunctionFailed. com.amazonaws.swf#LambdaFunctionStartedEventAttributes requires scheduledEventId. com.amazonaws.swf#LambdaFunctionCompletedEventAttributes requires scheduledEventId + startedEventId and carries result (Data, @length max 32768). com.amazonaws.swf#StartLambdaFunctionFailedCause = [ASSUME_ROLE_FAILED].

## Steps to Reproduce
1. iam: CreateRole R_exec trusting lambda.amazonaws.com, attach AWSLambdaBasicExecutionRole.
2. lambda: CreateFunction F (python3.12, handler returning {"sum": sum(event["numbers"])}) with Role=R_exec; wait for State=Active.
3. swf: RegisterDomain D (workflowExecutionRetentionPeriodInDays=1); RegisterWorkflowType W/1.0 in D with defaultTaskStartToCloseTimeout=60, defaultExecutionStartToCloseTimeout=600, defaultChildPolicy=TERMINATE.
4. iam: CreateRole R_invoke trusting swf.amazonaws.com with an inline policy allowing lambda:InvokeFunction on F's ARN.
5. swf: StartWorkflowExecution in D for W/1.0 on task list TL with lambdaRole=<R_invoke ARN>.
6. swf: PollForDecisionTask on D/TL until a taskToken is returned.
7. swf: RespondDecisionTaskCompleted with one decision {decisionType: "ScheduleLambdaFunction", scheduleLambdaFunctionDecisionAttributes: {id: "lam-1", name: F, input: '{"numbers":[1,2,3]}', startToCloseTimeout: "30"}}.
8. swf: poll GetWorkflowExecutionHistory for the execution for 30 s.

Observed: the history ends at LambdaFunctionScheduled and no LambdaFunctionStarted / LambdaFunctionCompleted / LambdaFunctionFailed / LambdaFunctionTimedOut / StartLambdaFunctionFailed / ScheduleLambdaFunctionFailed event ever appears; no follow-up DecisionTaskScheduled is created either.
Expected: LambdaFunctionStarted then LambdaFunctionCompleted with result '{"sum": 6}' (or a terminal Lambda failure event), followed by a new DecisionTaskScheduled.

Contrast run (passes today, included to isolate the branch): repeat steps 5-8 WITHOUT lambdaRole on StartWorkflowExecution — the emulator correctly records LambdaFunctionScheduled then StartLambdaFunctionFailed with cause ASSUME_ROLE_FAILED.
