Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__events__iam.ecs-target-without-role-arn-rejected ecs__events__iam.fargate-ecs-target-without-network-configuration-rejected ecs__events__iam.ecs-target-pinned-to-deregistered-revision-launch-blocked-until-repinned ecs__events__iam.pinned-revision-target-keeps-launching-old-revision-after-reregistration ecs__events__iam.ecs-target-assign-public-ip-governs-the-eventbridge-launched-tasks-real-public-ip ecs__events__iam.rule-and-task-definition-lookup-from-another-region-rejected
Issue-Key: ecs__events__iam.ecs-target-without-role-arn-rejected|_error#15|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error")
Issue-Key: ecs__events__iam.fargate-ecs-target-without-network-configuration-rejected|_error#14|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error")
Issue-Key: ecs__events__iam.ecs-target-pinned-to-deregistered-revision-launch-blocked-until-repinned|_await#17|Timeout|time.sleep(5)
Issue-Key: ecs__events__iam.pinned-revision-target-keeps-launching-old-revision-after-reregistration|run#21|Timeout|time.sleep(5)
Issue-Key: ecs__events__iam.ecs-target-assign-public-ip-governs-the-eventbridge-launched-tasks-real-public-ip|run#17|Timeout|time.sleep(5)
Issue-Key: ecs__events__iam.rule-and-task-definition-lookup-from-another-region-rejected|run#26|AssertionError|assert far_tds == [], f"ListTaskDefinitions in {other} must be empty, got {far_tds!r}"
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ecs__events__iam: EventBridge ECS targets are accepted without a RoleArn or networkConfiguration, pinned revisions are not honoured, launched tasks never settle, and task definitions leak across regions (6 causes)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Cause 1 of 6: PutTargets accepts an ECS target with no RoleArn.
AWS refuses it with ValidationException — an ecsParameters target requires a RoleArn EventBridge can assume to call RunTask. Emulator: the call succeeds.
Fix: the PutTargets ECS-target validation.
Trace: ecs__events__iam.ecs-target-without-role-arn-rejected.

Cause 2 of 6: PutTargets accepts a Fargate ECS target with no NetworkConfiguration.
AWS refuses it with ValidationException — a FARGATE launch type with an awsvpc task definition requires NetworkConfiguration. Emulator: the call succeeds.
Fix: the same PutTargets ECS-target validation.
Trace: ecs__events__iam.fargate-ecs-target-without-network-configuration-rejected.

Cause 3 of 6: a target pinned to a deregistered task-definition revision still launches (or rather, never reports being blocked).
AWS: a rule whose ecsParameters pin a deregistered revision cannot launch a task until it is re-pinned to an ACTIVE revision, and the failure is observable.
Emulator: the check polls until the 60s budget expires — neither the block nor the subsequent success is observable.
Fix: the EventBridge-to-ECS RunTask path must resolve the pinned revision and surface the failure.
Trace: ecs__events__iam.ecs-target-pinned-to-deregistered-revision-launch-blocked-until-repinned.

Cause 4 of 6: a target pinned to a specific revision does not keep launching that revision after the family is re-registered.
AWS: an explicitly pinned family:revision keeps launching that revision even after a newer one is registered.
Emulator: the check polls until the 60s budget expires.
Fix: the same revision resolution in the EventBridge ECS target path.
Trace: ecs__events__iam.pinned-revision-target-keeps-launching-old-revision-after-reregistration.

Cause 5 of 6: the assignPublicIp of an EventBridge-launched task has no observable effect.
AWS: with assignPublicIp=ENABLED the launched task's ENI gets a public IP; with DISABLED it does not.
Emulator: the check polls until the 60s budget expires (the launched task has no ENI details at all).
Fix: the ECS awsvpc ENI materialisation plus the EventBridge target's launch path.
Trace: ecs__events__iam.ecs-target-assign-public-ip-governs-the-eventbridge-launched-tasks-real-public-ip.

Cause 6 of 6: ECS task definitions are not region-scoped for ListTaskDefinitions.
AWS: a task definition registered in us-west-2 is invisible to ListTaskDefinitions in us-east-2, and a rule/task-definition lookup from another region is refused.
Emulator: ListTaskDefinitions in us-east-2 returns the us-west-2 ARN, i.e. the task-definition store is not partitioned by region.
Fix: the region partitioning of the ECS task-definition state/index.
Trace: ecs__events__iam.rule-and-task-definition-lookup-from-another-region-rejected.

## Evidence
1. `AssertionError: the call succeeded; real AWS rejects it, so there is no error`.
2. `AssertionError: the call succeeded; real AWS rejects it, so there is no error`.
3-5. `Error: Check timed out after 60.0s`.
6. `AssertionError: ListTaskDefinitions in us-east-2 must be empty, got ['arn:aws:ecs:us-west-2:000000000000:task-definition/sim2real-ebecs-<suffix>:1']` — note the returned ARN's region is us-west-2 while the call was made against us-east-2 (the family name is per-run).

## Steps to Reproduce
1. PutRule then PutTargets with an ECS target and no RoleArn -> accepted.
2. PutTargets with LaunchType=FARGATE on an awsvpc task definition and no NetworkConfiguration -> accepted.
3. Pin ecsParameters to family:1, DeregisterTaskDefinition family:1, PutEvents -> nothing observable.
4. Pin family:1, register family:2, PutEvents -> the launched revision is never observable.
5. Pin AssignPublicIp=ENABLED, PutEvents, inspect the launched task's ENI -> no details.
6. RegisterTaskDefinition in us-west-2, then ListTaskDefinitions in us-east-2 -> the us-west-2 ARN is returned.
