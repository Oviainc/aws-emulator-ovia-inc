Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__swf.schedule-lambda-function-without-role-rejected
Issue-Key: lambda__swf.schedule-lambda-function-without-role-rejected|_poll_until#37|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: bc4720188099afd2bb4f421d1e83533814225f44

# An SWF ScheduleLambdaFunction decision never completes: the Lambda task stays scheduled

> _Note: this issue was identified against the code base at commit `bc4720188099afd2bb4f421d1e83533814225f44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RespondDecisionTaskCompleted

## Description
Cause 1 of 1: a workflow whose decider answers with a ScheduleLambdaFunction decision gets a LambdaFunctionScheduled event and then nothing — the function is never started and the workflow never settles.

AWS: when a decider returns a ScheduleLambdaFunction decision on a workflow started with a lambdaRole, SWF records LambdaFunctionScheduled, assumes the workflow's lambdaRole, invokes the function, and records LambdaFunctionStarted followed by LambdaFunctionCompleted (or LambdaFunctionFailed / LambdaFunctionTimedOut), after which the next decision task is scheduled and the workflow can close. A workflow started without a lambdaRole is instead refused for the scheduling decision — which is the behavior under test, and which needs the positive control to settle first.

Emulator: the control workflow's history stops at `LambdaFunctionScheduled`; no LambdaFunctionStarted/Completed/Failed event is ever recorded and no further decision task is scheduled, so the workflow does not settle within the wait window. (Recorded history ends at event id 5, the LambdaFunctionScheduled event, after WorkflowExecutionStarted / DecisionTaskScheduled / DecisionTaskStarted / DecisionTaskCompleted.)

Why: SWF's Lambda task support is unusable — a decider that schedules a Lambda function waits forever with a history that shows the task accepted, so a client cannot tell a stuck emulator from a slow function, and the no-lambdaRole refusal cannot be observed at all.

Fix: the SWF decision executor for the ScheduleLambdaFunction decision — assume the workflow's lambdaRole, invoke the function, and write the LambdaFunctionStarted/Completed/Failed/TimedOut history events plus the follow-on decision task; the lambdaRole precondition belongs in the same path.

Traces: lambda__swf.schedule-lambda-function-without-role-rejected.

## Evidence
Check lambda__swf.schedule-lambda-function-without-role-rejected:
AssertionError: control attempt 1 to settle not met within 30s. Last result: [WorkflowExecutionStarted (eventId 1, lambdaRole 'arn:aws:iam::<account>:role/<path>/<role>'), DecisionTaskScheduled (2), DecisionTaskStarted (3), DecisionTaskCompleted (4), LambdaFunctionScheduled (5, id 'sim2real-lam...')]
— the history stops at LambdaFunctionScheduled; no LambdaFunctionStarted/Completed event follows.
(workflow/role/task-list names, timestamps and the 30 s budget are per-run values; the invariant is that the scheduled Lambda task never progresses.)

## Steps to Reproduce
1. Register an SWF domain, workflow type and activity/task list, and create a Lambda function.
2. StartWorkflowExecution with a lambdaRole that may invoke the function.
3. Poll for a decision task and RespondDecisionTaskCompleted with a ScheduleLambdaFunction decision naming the function.
4. Poll GetWorkflowExecutionHistory: the history stops at LambdaFunctionScheduled and the workflow never settles.
