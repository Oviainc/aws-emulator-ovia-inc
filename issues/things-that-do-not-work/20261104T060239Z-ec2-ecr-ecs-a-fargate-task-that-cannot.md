Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecr__ecs.ecsdeploy-isolated-subnet-task-blocked-from-starting ec2__ecr__ecs.ecsnrt-fargate-task-on-unpushed-tag-fails-pull-until-retagged
Issue-Key: ec2__ecr__ecs.ecsdeploy-isolated-subnet-task-blocked-from-starting|_poll#28|Timeout|return last
Issue-Key: ec2__ecr__ecs.ecsnrt-fargate-task-on-unpushed-tag-fails-pull-until-retagged|_poll#19|Timeout|return last
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ec2__ecr__ecs: a Fargate task that cannot initialise or cannot pull its image never reaches a terminal state (1 cause)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
One net-new root cause, grounded in failing behavioral checks under the suite heading `ec2__ecr__ecs`.

Cause 1 of 1: a Fargate task that cannot start never settles into STOPPED with a stop code.
AWS: Smithy: ecs#Task carries `lastStatus` (Task Lifecycle), `stopCode` (ecs#TaskStopCode, "The stop code indicating why a task was stopped") and `stoppedReason`. A task placed in a subnet with no route to the ECR/ECS endpoints stops with stopCode=TaskFailedToStart and a ResourceInitializationError stoppedReason; a task whose image reference names a tag that was never pushed stops with stopCode=TaskFailedToStart and a CannotPullContainerError stoppedReason, and the same task definition starts normally once the tag exists. The precise reason strings are recorded behavior only; the requirement under test is that the task reaches STOPPED with a stop code at all.
Emulator: DescribeTasks never reports a terminal task for either scenario, so both checks exhaust their poll budget ("Check timed out after 60.0s") without ever seeing a stopCode or stoppedReason. Tasks are accepted and then stay non-terminal regardless of whether their network or their image can actually be resolved.
Why: a client cannot distinguish "still starting" from "failed to start": retry/rollback logic keyed on stopCode=TaskFailedToStart never triggers, and an unreachable-subnet or missing-tag deployment looks like a slow but healthy one forever.
Fix: the ECS task state machine behind RunTask/DescribeTasks must evaluate placement (subnet reachability) and image resolution and drive the task to STOPPED with stopCode/stoppedReason (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src). The open issue 20261103T021956Z-ec2-ecr-ecs-ecs-fargate-ephemeral-storage-bounds-unenforced-services-are.md covers other causes under this same heading and does not carry these two checks.
Traces: ec2__ecr__ecs.ecsdeploy-isolated-subnet-task-blocked-from-starting, ec2__ecr__ecs.ecsnrt-fargate-task-on-unpushed-tag-fails-pull-until-retagged.

## Evidence
Both checks fail with "Check timed out after 60.0s" while polling DescribeTasks for a terminal task (the isolated-subnet check budgets 720 s for the STOPPED/TaskFailedToStart transition; the unpushed-tag check waits for the pull failure and then for success after retagging). Expected: lastStatus=STOPPED with stopCode=TaskFailedToStart and a ResourceInitializationError / CannotPullContainerError stoppedReason. Actual: no terminal state is ever reported.

## Steps to Reproduce
1. Create a VPC with an isolated subnet (no IGW/NAT, no ECR/S3 endpoints), create a cluster and a FARGATE service/task in that subnet, then poll DescribeTasks - the task never reaches STOPPED with stopCode=TaskFailedToStart.
2. Push an image under one tag, register a task definition referencing a second tag that was never pushed, RunTask and poll DescribeTasks - the task never stops with a CannotPullContainerError stoppedReason, and the retag-then-succeed half of the scenario is never reached.
