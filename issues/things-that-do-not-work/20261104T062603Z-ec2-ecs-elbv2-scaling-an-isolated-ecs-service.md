Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecs__elbv2.ecsnt-scaling-the-isolated-service-up-stops-its-task-before-any-target-registers
Issue-Key: ec2__ecs__elbv2.ecsnt-scaling-the-isolated-service-up-stops-its-task-before-any-target-registers|_poll#12|Timeout|last = probe()
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ec2__ecs__elbv2: scaling an isolated ECS service up never stops its task or registers a target, so the scenario never settles (1 cause)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateService

## Description
Cause 1 of 1: an ECS service whose tasks cannot reach the network neither progresses nor terminates when it is scaled up.

AWS: scaling the isolated service's desiredCount up launches a task that fails to start (no route to the registry), so the task is stopped with a start failure before any target is registered in the target group — both the stop and the empty target list are observable.

Emulator: the check polls until the 60s budget expires. The task neither starts nor stops and no target registration (or absence of one) can be confirmed, because the ECS task runtime never produces a terminal outcome and the service never registers targets with ELBv2.

Why: a client cannot distinguish "the task failed to start, so nothing was registered" from "the task is still coming up" — the diagnosis this scenario exists to support is impossible.

Fix: the ECS task runtime's start/pull failure path (record stopCode/stoppedReason) and the ECS service to ELBv2 target-registration path.

Traces: ec2__ecs__elbv2.ecsnt-scaling-the-isolated-service-up-stops-its-task-before-any-target-registers.

## Evidence
Expected (AWS): the newly scheduled task reaches lastStatus=STOPPED with a start-failure stopCode/stoppedReason and DescribeTargetHealth shows no registered target for it.
Actual (emulator): `Error: Check timed out after 60.0s`.

## Steps to Reproduce
1. Create an ALB target group and an ECS service in an isolated subnet with loadBalancers configured, desiredCount=0.
2. UpdateService desiredCount=1.
3. Poll DescribeTasks and DescribeTargetHealth: the task never reaches a terminal status and no target state can be observed.
