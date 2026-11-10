Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__stepfunctions.metrics-states-stream-late-activity-result-rejected-after-task-timeout-and-only-timed-out-moves
Issue-Key: cloudwatch__stepfunctions.metrics-states-stream-late-activity-result-rejected-after-task-timeout-and-only-timed-out-moves|_poll#31|Timeout|time.sleep(_POLL_S)
Issue-Commit: 5598dd881b8e225c34bc5a9814ddc1ff981b448f

# cloudwatch__stepfunctions: an activity Task never hands out a token, so a worker's late answer is never refused with TaskTimedOut and neither ActivitiesTimedOut nor ExecutionsFailed is ever published

> _Note: this issue was identified against the code base at commit `5598dd881b8e225c34bc5a9814ddc1ff981b448f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetActivityTask

## Description
One net-new root cause, grounded in one failing behavioral check the replay report left unmarked (no ALREADY-FILED marker and no OWNED marker).

Cause 1 of 1 — an execution whose Task state names an activity ARN never enqueues a task, so GetActivityTask returns no taskToken; the Task's TimeoutSeconds never lapses, a worker's late SendTaskSuccess is never refused, and no AWS/States datapoint for the activity or the execution is ever published.
Failing check id: cloudwatch__stepfunctions.metrics-states-stream-late-activity-result-rejected-after-task-timeout-and-only-timed-out-moves.
AWS: Smithy (services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model/com-amazonaws-sfn-awsstepfunctions.json) models GetActivityTask with input {activityArn (required, @length 1..256), workerName (@length 1..80)} and output {taskToken (@length 1..2048, @sensitive), input (@length 0..1048576, @sensitive)}; modeled errors ActivityDoesNotExist, ActivityWorkerLimitExceeded, InvalidArn, KmsAccessDeniedException, KmsInvalidStateException, KmsThrottlingException, all HTTP 400. In AWS the task is enqueued the moment the execution enters the Task state and the next long-poll returns a token with the state input. Recorded behavior for the rest: a worker that answers AFTER the Task's TimeoutSeconds has elapsed is refused with the TaskTimedOut error class and its message for that class, the execution is already FAILED, and in namespace AWS/States only ActivitiesTimedOut (Dimensions exactly [ActivityArn]) and ExecutionsFailed (Dimensions exactly [StateMachineArn]) move, while an in-time control activity moves the succeeded counters instead.
Emulator: the check is killed by the harness with "Error: Check timed out after 600.0s" while still polling for its first activity task, so no token, no timeout transition, no late-answer refusal and no GetMetricStatistics datapoint is ever produced; both executions stay RUNNING.
Why: a client built on the activity-worker pattern gets no work and no error saying why, and its executions hang in RUNNING. Operational tooling that alarms on AWS/States ActivitiesTimedOut / ExecutionsFailed reads an empty namespace, so an abandoned worker is invisible, and a worker cannot distinguish "my answer was too late" from "the service lost it".
Fix: the activity task scheduling path shared by the interpreter's Task-state handler and the GetActivityTask handler/state, then the Task timeout timer, the SendTaskSuccess token-validity check that must refuse a post-timeout answer, and the AWS/States metric emission for the [ActivityArn] and [StateMachineArn] dimension families.
Related but NOT covering this check (the replay report marked it owned by nothing): open issues 20261025T100143Z-cloudwatch-sfn-activity-backed-task-states-never-enqueue.md and 20261025T075213Z-cloudwatch-sfn-an-activity-backed-task-state-never.md.
Traces: cloudwatch__stepfunctions.metrics-states-stream-late-activity-result-rejected-after-task-timeout-and-only-timed-out-moves.

## Evidence
cloudwatch__stepfunctions.metrics-states-stream-late-activity-result-rejected-after-task-timeout-and-only-timed-out-moves — verdict mismatch.
Expected: two STANDARD executions, one on an activity Task with TimeoutSeconds=5 and one control with TimeoutSeconds=300; GetActivityTask returns a non-empty taskToken for each within seconds; the control's in-time SendTaskSuccess drives its execution to SUCCEEDED; the subject's Task times out on its own so the execution becomes FAILED; the worker's later SendTaskSuccess on the stale token is refused with the TaskTimedOut error and its message; AWS/States then shows ActivitiesTimedOut under Dimensions exactly [ActivityArn=<subject>] and ExecutionsFailed under Dimensions exactly [StateMachineArn=<subject sm>] moving, with the succeeded counters moving only for the control.
Actual: "Error: Check timed out after 600.0s" — the harness killed the check inside its first GetActivityTask poll. No taskToken was ever returned, so SendTaskSuccess, the FAILED transition, the late-answer refusal and every metric read were unreachable.

## Steps to Reproduce
1. iam:CreateRole with a trust policy for states.amazonaws.com.
2. sfn:CreateActivity twice -> <slow_act>, <ctrl_act>.
3. sfn:CreateStateMachine (STANDARD) twice: {"StartAt":"Work","States":{"Work":{"Type":"Task","Resource":"<act>","TimeoutSeconds":5 | 300,"End":true}}}.
4. sfn:StartExecution on both.
5. sfn:GetActivityTask(activityArn=<slow_act>) and (activityArn=<ctrl_act>) — expected: a non-empty taskToken each, within seconds. Observed: none for the whole 600 s window; the check is killed at the harness cap.
6. (unreachable today) sfn:SendTaskSuccess with the control token -> control execution SUCCEEDED.
7. (unreachable today) wait past the subject Task's TimeoutSeconds -> DescribeExecution = FAILED; then sfn:SendTaskSuccess with the stale subject token -> refused with the TaskTimedOut error and its message.
8. (unreachable today) cloudwatch:GetMetricStatistics in namespace AWS/States for ActivitiesTimedOut with Dimensions exactly [ActivityArn=<slow_act>] and ExecutionsFailed with Dimensions exactly [StateMachineArn=<slow sm>]; only those (plus the control's succeeded counters) must move.
