Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ecs__elbv2.cli-create-alb-target-group-ecs-service-e2e autoscaling__ecs__elbv2.ecs-service-deregisters-tasks-on-scale-down
Issue-Key: autoscaling__ecs__elbv2.cli-create-alb-target-group-ecs-service-e2e|_poll_until#36|Timeout|last = probe()
Issue-Key: autoscaling__ecs__elbv2.ecs-service-deregisters-tasks-on-scale-down|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Commit: 68ec0c9cae77cfbc133f0186a4c923306bef5402

# autoscaling__ecs__elbv2: 2 net-new causes — an ECS service never registers its awsvpc task as an ELBv2 target, and scaling to 0 never drains/deregisters it

> _Note: this issue was identified against the code base at commit `68ec0c9cae77cfbc133f0186a4c923306bef5402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
Two net-new root causes in the ECS/ELBv2 target-registration lifecycle. Both are grounded in checks the replay report did NOT mark ALREADY FILED or OWNED. The open issue 20261104T042502Z-autoscaling-ecs-elbv2-6-net-new-root-causes.md covers adjacent causes in this service but does not carry these two replay keys.

Cause 1 of 2: an ECS service with a loadBalancers entry never registers its running task as a target.
AWS: ECS CreateService with loadBalancers=[{targetGroupArn, containerName, containerPort}] on an awsvpc/FARGATE task definition drives the service to runningCount=desiredCount, attaches an ElasticNetworkInterface attachment to each task whose details carry networkInterfaceId / privateIPv4Address (Smithy: ecs#Attachment.details — "For elastic network interfaces, this includes the network interface ID, the MAC address, the subnet ID, and the private IPv4 address"; ecs#NetworkInterface.privateIpv4Address), and registers that private IPv4 address as an `ip` target of the named target group, so elbv2 DescribeTargetHealth returns exactly one TargetHealthDescription for it and the ALB listener forwards to it. Nothing in the ECS model describes the convergence timing, so beyond the modeled attachment shape this is recorded behavior only: the recording reaches a registered, healthy target and a 200 body from the ALB DNS name.
Emulator: the state the check polls for is never reached — the whole check is killed by the harness with "Check timed out after 60.0s" while still polling for the task's ENI private IP to appear as a registered target. No target is ever registered, so no verdict on health or routing is reachable.
Why: a client that creates a service behind an ALB and then waits on DescribeTargetHealth (the documented way to know a deployment is serving) waits forever; every CLI/CDK/CFN rollout gate built on "wait until targets healthy" hangs rather than converging or failing.
Fix: ECS service-scheduler state plus the ECS→ELBv2 target-registration side effect: the task attachment must be populated with a real ENI id/private IPv4, and reaching RUNNING must register that address with each loadBalancers target group.
Traces: autoscaling__ecs__elbv2.cli-create-alb-target-group-ecs-service-e2e.

Cause 2 of 2: scaling a service to desiredCount=0 never drains and never deregisters the target.
AWS: UpdateService desiredCount=0 stops the task; the target it registered moves to TargetHealth.State='draining' with Reason='Target.DeregistrationInProgress' and is only removed from DescribeTargetHealth after the target group's deregistration_delay.timeout_seconds elapses. Recorded behavior only for the timing; the draining state and the delay are AWS's documented deregistration semantics.
Emulator: the check never observes either phase and is killed with "Check timed out after 60.0s". Because Cause 1 means no target is ever registered, there is nothing to drain; but the deregistration-delay behaviour is a separate missing mechanism and must be fixed on top of Cause 1 before this check can pass.
Why: blue/green and scale-in automation that waits for a target to leave 'draining' before tearing down infrastructure never proceeds, and callers cannot distinguish "still draining" from "never registered".
Fix: the ECS service-scheduler scale-in path and the ELBv2 target-group deregistration state machine (draining state + deregistration_delay honouring).
Traces: autoscaling__ecs__elbv2.ecs-service-deregisters-tasks-on-scale-down.

## Evidence
Expected (AWS): after CreateService with a loadBalancers entry, elbv2 DescribeTargetHealth returns the task's ENI private IPv4 as a registered target and the ALB returns the task's HTTP body; after UpdateService desiredCount=0 that target reports State='draining' and then disappears once the deregistration delay elapses.
Actual (emulator): both checks are terminated by the harness with `Check timed out after 60.0s` while still polling — no target is ever registered, so neither registration, health, routing, draining nor deregistration is ever observable.

## Steps to Reproduce
1. Create a VPC/subnets/SG, an internet-facing ALB and an `ip`-type target group (deregistration_delay.timeout_seconds=10).
2. RegisterTaskDefinition with networkMode=awsvpc, requiresCompatibilities=[FARGATE].
3. CreateService with desiredCount=1 and loadBalancers=[{targetGroupArn, containerName, containerPort}].
4. Poll elbv2 DescribeTargetHealth for the target group → no target ever appears (check 'cli-create-alb-target-group-ecs-service-e2e' times out at 60s).
5. UpdateService desiredCount=0 and poll DescribeTargetHealth → no 'draining' state and no removal is ever observed (check 'ecs-service-deregisters-tasks-on-scale-down' times out at 60s).
