Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy__ecs__elbv2.ecs-canary-deployment-shifts-traffic-in-stages
Issue-Key: codedeploy__ecs__elbv2.ecs-canary-deployment-shifts-traffic-in-stages|_poll_until#16|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s (last={last!r})")
Issue-Commit: 74ebb92ce9e8b3d0346604c0e135c3e0968fb0a4

# codedeploy__ecs__elbv2: an ECS service backing a CodeDeploy canary deployment never reaches runningCount==desiredCount, so the staged traffic shift cannot start (1 cause)

> _Note: this issue was identified against the code base at commit `74ebb92ce9e8b3d0346604c0e135c3e0968fb0a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: an ECS service created with launchType/desiredCount=1 behind an ELBv2 target group, as the blue side of a CodeDeploy ECS blue/green deployment, never reports a running task.
AWS: after ecs:CreateService with desiredCount=1 and a healthy task definition, the service places a task and ecs:DescribeServices reports runningCount rising to desiredCount (1) within a couple of minutes; only then can CodeDeploy's ECS blue/green deployment create the green task set and shift listener traffic in canary stages (TimeBasedCanary: canaryPercentage first, then the remainder after canaryInterval minutes).
Emulator: DescribeServices keeps reporting runningCount 0 for the whole wait — "service sim2real-xsvc-svc-… runningCount==1 not met within 360.0s (last=0)" — so the check never gets to CreateDeployment and no part of the staged ECS traffic shift is observable. The service name is a per-run value.
Why: the entire CodeDeploy ECS blue/green surface is unreachable for a client: no task set creation, no listener weight staging, no canary/rollback behavior. Because the symptom is a steady 0 rather than an error, client wait loops hang instead of failing fast.
Fix: the ECS service/task placement state must advance a service's runningCount to desiredCount (placing and reporting tasks, and registering them with the target group) so the CodeDeploy ECS blue/green deployment driver has a blue task set to shift from. This is the same symptom as open issue 20261104T062603Z-codedeploy-ecs-elbv2-an-ecs-service-s-runningcount.md, which covers the listener-move check and does not carry this check's replay key.
Traces: codedeploy__ecs__elbv2.ecs-canary-deployment-shifts-traffic-in-stages.

## Evidence
Expected (AWS): ecs:DescribeServices on the newly created service reports runningCount == desiredCount (1) within the 360s budget, after which codedeploy:CreateDeployment with an ECS blue/green AppSpec stages listener weights (canary percentage, then 100%).

Actual (emulator): "AssertionError: service sim2real-xsvc-svc-01060dfdf26e runningCount==1 not met within 360.0s (last=0)" raised from _wait_running_count/_poll_until — runningCount stayed 0 for the whole poll, so the deployment was never created.

## Steps to Reproduce
1. Create a VPC/subnets, an ALB with a listener and two target groups, an ECS cluster and a task definition.
2. ecs:CreateService with desiredCount=1, DeploymentController type CODE_DEPLOY, and the blue target group.
3. Poll ecs:DescribeServices(cluster, services=[<svc>]) for runningCount — observe it stays 0 indefinitely (desiredCount 1).
4. Because the blue side never runs, codedeploy:CreateDeployment for the ECS blue/green canary config is never attempted and no listener weight staging can be observed.
