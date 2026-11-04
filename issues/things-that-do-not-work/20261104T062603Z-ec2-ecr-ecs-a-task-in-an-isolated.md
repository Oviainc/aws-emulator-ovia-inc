Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecr__ecs.ecsdeploy-isolated-subnet-task-blocked-from-starting ec2__ecr__ecs.ecsnrt-fargate-task-on-unpushed-tag-fails-pull-until-retagged
Issue-Key: ec2__ecr__ecs.ecsdeploy-isolated-subnet-task-blocked-from-starting|_poll#28|Timeout|return last
Issue-Key: ec2__ecr__ecs.ecsnrt-fargate-task-on-unpushed-tag-fails-pull-until-retagged|_poll#17|Timeout|time.sleep(interval_s)
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ec2__ecr__ecs: a task in an isolated subnet is never blocked from starting and an unpullable image tag never fails the task (2 causes)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Cause 1 of 2: a Fargate task placed in a subnet with no route to the registry neither starts nor fails.
AWS: the task cannot pull its image, so it stops with a CannotPullContainerError stopped reason (stopCode TaskFailedToStart) — an observable terminal outcome.
Emulator: the check polls until the 60s budget expires; the task neither starts nor stops, so the isolation has no effect at all.
Fix: the ECS task runtime's image-pull step must consult the task's network placement and record the resulting stop code/reason.
Trace: ec2__ecr__ecs.ecsdeploy-isolated-subnet-task-blocked-from-starting.

Cause 2 of 2: a task referencing an ECR tag that has not been pushed never fails, and never succeeds once the tag is pushed.
AWS: the first task stops with a pull failure naming the missing tag; after the image is re-tagged and pushed, an identical task runs.
Emulator: the check polls until the 60s budget expires — neither the failure nor the post-push success is observable.
Fix: the same image-pull step, resolving the tag against the ECR repository's image index.
Trace: ec2__ecr__ecs.ecsnrt-fargate-task-on-unpushed-tag-fails-pull-until-retagged.

Why (both): image-pull failure is the single most common real ECS failure mode. A client that reads stoppedReason/stopCode to diagnose a broken rollout gets nothing, and the emulator cannot distinguish a working image reference from a broken one.

## Evidence
Both checks end with `Error: Check timed out after 60.0s`. Expected (AWS): DescribeTasks eventually reports lastStatus=STOPPED with stopCode=TaskFailedToStart and a stoppedReason describing the pull failure (e.g. CannotPullContainerError naming the image reference from the request); after the tag is pushed the same request reaches RUNNING.

## Steps to Reproduce
1. Create a VPC subnet with no route to the registry/no NAT; RunTask Fargate with that subnet -> the task never starts and never stops.
2. Create an ECR repository without pushing tag 'v2'; RunTask with image <repo>:v2 -> no pull failure; push/re-tag v2 and repeat -> no success either, both polls time out.
