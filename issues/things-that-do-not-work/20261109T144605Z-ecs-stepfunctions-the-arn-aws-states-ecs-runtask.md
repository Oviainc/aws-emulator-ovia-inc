Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__stepfunctions.ecs-run-task-sync-resolves-task-token-into-container-env ecs__stepfunctions.ecs-run-task-sync-without-run-task-permission-denied-and-no-task-created
Issue-Key: ecs__stepfunctions.ecs-run-task-sync-resolves-task-token-into-container-env|_run_until_task_scheduled#8|AssertionError|raise AssertionError(
Issue-Key: ecs__stepfunctions.ecs-run-task-sync-without-run-task-permission-denied-and-no-task-created|run#11|AssertionError|assert negative_desc.get("error") == "ECS.AccessDeniedException", (
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# ecs__stepfunctions: the arn:aws:states:::ecs:runTask.sync integration is unsupported — the state machine fails with States.Runtime instead of running (and denying) an ECS task

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: a Step Functions task state whose Resource is `arn:aws:states:::ecs:runTask.sync` is refused by the emulator's own runtime, so no ECS task is ever started and neither the success path nor the authorization-failure path of the integration exists.

AWS: recorded behavior only for the integration surface; the modeled surface is sfn StartExecution/DescribeExecution/GetExecutionHistory and ecs RunTask. On real AWS the optimized `ecs:runTask.sync` (run-a-job) integration calls ECS RunTask, waits for the task to reach STOPPED, and surfaces the task result to the state. Two consequences the checks assert: (a) the service-integration task token / resolved parameters reach the container's environment, and (b) when the state machine's execution role lacks ecs:RunTask, the execution fails with the error code `ECS.AccessDeniedException` (the integration surfaces the ECS refusal as `<Service>.<ErrorName>`), and no ECS task is created.

Emulator: both executions end FAILED with error `States.Runtime` and cause `state 'RunTask': 'arn:aws:states:::ecs:runTask.sync' asks for the run-a-job ('sync') pattern of an integration whose job this emulator cannot report or stop, so no job was started and nothing was run for it`. That cause text is emulator-authored, not an AWS error: `AssertionError: execution ended 'FAILED' before scheduling its task` and `AssertionError: expected error 'ECS.AccessDeniedException', got 'States.Runtime'`.

Why: the single most common way to run an ECS task from a workflow is unusable. A client cannot test a synchronous ECS job step at all, and — worse for correctness work — an authorization failure it *should* see as `ECS.AccessDeniedException` is masked as a generic runtime error, so retry/catch rules written on the AWS error names (`States.TaskFailed`, `ECS.AccessDeniedException`) never match.

Fix: the Step Functions service-integration dispatcher for the `.sync` (run-a-job) pattern over ECS RunTask — start the ECS task, poll it to STOPPED, report its result to the state, and map an ECS refusal onto the `ECS.<ErrorName>` execution error instead of answering States.Runtime.

Traces: ecs__stepfunctions.ecs-run-task-sync-resolves-task-token-into-container-env, ecs__stepfunctions.ecs-run-task-sync-without-run-task-permission-denied-and-no-task-created.

## Evidence
Expected: a state with Resource='arn:aws:states:::ecs:runTask.sync' runs an ECS task to completion and the execution succeeds; with the execution role lacking ecs:RunTask, the execution fails with error 'ECS.AccessDeniedException' and ECS has no task.
Actual: `execution ended 'FAILED' before scheduling its task: 'States.Runtime' "state 'RunTask': 'arn:aws:states:::ecs:runTask.sync' asks for the run-a-job ('sync') pattern of an integration whose job this emulator cannot report or stop, so no job was started and nothing was run for it"`; and `expected error 'ECS.AccessDeniedException', got 'States.Runtime'`.

## Steps to Reproduce
1. ecs RegisterTaskDefinition (Fargate) and create a cluster.
2. sfn CreateStateMachine with a Task state: Resource='arn:aws:states:::ecs:runTask.sync', Parameters naming the cluster, task definition and a container override carrying the task token.
3. sfn StartExecution and poll DescribeExecution / GetExecutionHistory.
4. Repeat with an execution role that does not allow ecs:RunTask.
Observed: both executions end FAILED with error 'States.Runtime' and no ECS task is created.
