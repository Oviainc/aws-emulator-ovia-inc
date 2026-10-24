Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__ecs.ecsnrt-empty-provider-run-task-rejected-unless-managed-scaling-parks-it
Issue-Key: autoscaling__ec2__ecs.ecsnrt-empty-provider-run-task-rejected-unless-managed-scaling-parks-it|run#21|AssertionError|assert got == [("PROVISIONING", on)], f"the managed provider's RunTask returned {got!r}"
Issue-Commit: 8839c6cff18de024fc6b68ddfb2d1b408a6dd709

# ECS RunTask through a managed-scaling capacity provider returns lastStatus PENDING instead of PROVISIONING for a task parked on empty capacity

> _Note: this issue was identified against the code base at commit `8839c6cff18de024fc6b68ddfb2d1b408a6dd709`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
One root cause (Cause 1 of 1).

Cause 1 of 1: RunTask placed through an ECS capacity provider that has managed scaling ENABLED, backed by an Auto Scaling group with Min=Max=Desired=0 (i.e. no container instance exists yet), returns the accepted task with `lastStatus` = `PENDING`. The task should be parked in `PROVISIONING` until managed scaling produces capacity.

AWS: recorded behavior only — the Smithy model does not pin the value (ECS `Task.lastStatus` is documented only as "The last known status for the task", pointing at the Task Lifecycle doc; `RunTask` returns `tasks`/`failures` with no modeled initial-status constraint). The recording shows that when RunTask is accepted through a capacity provider whose managed scaling is ENABLED but whose group currently holds no container instances, the returned task carries `lastStatus` = `PROVISIONING` and `capacityProviderName` = that provider: the task is parked awaiting capacity, not yet placed. The sibling refusal paths in the same recording are already faithful — RunTask through the managed-scaling-DISABLED empty provider is refused `InvalidParameterException` (HTTP 400), as is RunTask with `launchType=EC2` on an instance-less cluster, and neither leaves a task behind — so the divergence is isolated to the accepted task's initial lifecycle state.

Emulator: the same accepted RunTask returns `[('PENDING', '<provider-name>')]` — the provider attribution is correct, the lifecycle state is not. `PROVISIONING` is never produced for a capacity-provider placement awaiting managed scaling; the emulator appears to initialise every accepted task directly at `PENDING`.

Why: `PROVISIONING` and `PENDING` mean different things to a client. `PROVISIONING` says ECS still has to acquire resources (here: an instance from the managed-scaling group) and the task has not been placed; `PENDING` says placement already happened and only container start is outstanding. A client (or CDK/ECS deploy wait loop) that polls for the provisioning phase, or that treats `PENDING` as evidence that capacity exists, draws the wrong conclusion: it will never observe the PROVISIONING phase at all, and a task that is in fact blocked on zero capacity looks like a task about to run. It also makes the one-flag distinction this check exercises invisible — the managed-scaling-ENABLED provider's parked task is reported in the same state an already-placed task would be.

Fix: the RunTask handler's task-creation path and the task state machine it writes — the initial `lastStatus` must be `PROVISIONING` when the placement is routed through a capacity provider that has to provision capacity (managed scaling ENABLED and no eligible container instance), and only transition to `PENDING` once the task is placed on an instance.

Traces: autoscaling__ec2__ecs.ecsnrt-empty-provider-run-task-rejected-unless-managed-scaling-parks-it (mismatch; assertion `got == [("PROVISIONING", on)]` failed with `[('PENDING', 'sim2real-ecsnrt-cp-...')]`).

## Evidence
Check: autoscaling__ec2__ecs.ecsnrt-empty-provider-run-task-rejected-unless-managed-scaling-parks-it [mismatch]

Expected (AWS, from the recording): RunTask accepted through the managed-scaling-ENABLED capacity provider returns one task with
  (lastStatus, capacityProviderName) == ("PROVISIONING", "<the managed provider>")
and `failures` == [].

Actual (emulator):
  AssertionError: the managed provider's RunTask returned [('PENDING', 'sim2real-ecsnrt-cp-8b929e39218b')]

So `capacityProviderName` matches and `failures` is empty, but `lastStatus` is `PENDING` where AWS reports `PROVISIONING`.

Already faithful in the same check (passed before the failing assertion):
  - RunTask via the managed-scaling-DISABLED empty provider -> InvalidParameterException (HTTP 400), message of the form "The specified capacity provider '<name>' contains no container instances."
  - RunTask with launchType=EC2 on the instance-less cluster -> InvalidParameterException (HTTP 400), message of the form "No Container Instances were found in your cluster."
  - ListTasks after the refusals returns [].

Smithy (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113): RunTask -> RunTaskRequest/RunTaskResponse {tasks, failures}; Task.lastStatus is documented only as "The last known status for the task" with a pointer to the ECS Task Lifecycle page, so the specific value is recorded behavior, not a modeled constraint.

## Steps to Reproduce
1. ec2.CreateVpc + CreateSubnet (one AZ).
2. ec2.CreateLaunchTemplate, then autoscaling.CreateAutoScalingGroup with MinSize=0, MaxSize=0, DesiredCapacity=0 and VPCZoneIdentifier=<subnet>. Do this twice (two groups).
3. ecs.CreateCapacityProvider for group A with autoScalingGroupProvider.managedScaling.status=DISABLED (call it `off`), and for group B with managedScaling.status=ENABLED, targetCapacity=100 (call it `on`).
4. ecs.CreateCluster, then ecs.PutClusterCapacityProviders(cluster, capacityProviders=[off, on]) and wait for attachmentsStatus=UPDATE_COMPLETE.
5. ecs.RegisterTaskDefinition with requiresCompatibilities=["EC2"] and one container (memoryReservation=64).
6. ecs.RunTask(cluster, taskDefinition, capacityProviderStrategy=[{capacityProvider: on, weight: 1}]).
7. Inspect the response: tasks[0].lastStatus.
   Expected: "PROVISIONING" (with capacityProviderName == `on`, failures == []).
   Observed: "PENDING".
