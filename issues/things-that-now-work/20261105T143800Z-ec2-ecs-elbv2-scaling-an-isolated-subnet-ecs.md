Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecs__elbv2.ecsnt-scaling-the-isolated-service-up-stops-its-task-before-any-target-registers
Issue-Key: ec2__ecs__elbv2.ecsnt-scaling-the-isolated-service-up-stops-its-task-before-any-target-registers|_poll#12|Timeout|time.sleep(interval_s)
Issue-Commit: 68ec0c9cae77cfbc133f0186a4c923306bef5402

# ec2__ecs__elbv2: scaling an isolated-subnet ECS service to 1 neither stops the task with TaskFailedToStart nor reports target health, so the scenario never settles

> _Note: this issue was identified against the code base at commit `68ec0c9cae77cfbc133f0186a4c923306bef5402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateService

## Description
Cause 1 of 1: a Fargate task placed in subnets with no route to the image registry never reaches a terminal stopped state, and the target group never reports a verdict.
AWS: UpdateService desiredCount=1 on an ALB-attached service whose awsvpc subnets are isolated (no NAT/IGW, no registry endpoint) starts a task that cannot pull its image: the task reaches STOPPED with stopCode='TaskFailedToStart' and a stoppedReason naming CannotPullContainerError (the fixed part is the stopCode value and the CannotPullContainerError class; the message's variable parts are the image reference and the underlying network error, taken from the request and the attempt). The target group never gains a healthy target. The identical task definition in a public subnet reaches RUNNING, which is the control that proves the subnets, not the definition, are the blocker. Recorded behavior only for timing; stopCode/stoppedReason are AWS's documented task-failure representation.
Emulator: the check is killed with "Check timed out after 60.0s" while still polling — the isolated task never reaches a STOPPED state carrying stopCode='TaskFailedToStart', the target group never reports any target health, and the public-subnet control never reaches RUNNING either, so no side of the comparison is reachable.
Why: a client diagnosing "my service will not start" gets no stopCode and no stoppedReason to act on — the single most important diagnostic ECS publishes for a failed launch is absent, and deploy gates that wait for either RUNNING or a terminal STOPPED hang indefinitely.
Fix: the ECS task-launch path must evaluate image pullability against the task's network placement and settle the task into STOPPED with stopCode='TaskFailedToStart' and a CannotPullContainerError stoppedReason; the ELBv2 target-health view for the service must reflect the absence of a running task.
Traces: ec2__ecs__elbv2.ecsnt-scaling-the-isolated-service-up-stops-its-task-before-any-target-registers.

This check was not marked ALREADY FILED or OWNED. The open issue 20261104T062603Z-ec2-ecs-elbv2-scaling-an-isolated-ecs-service.md describes an adjacent cause under this heading but does not carry this replay key; 20261102T085002Z-ec2-ecs-elbv2-3-net-new-ecs-root.md covers three other causes.

## Evidence
Expected (AWS): the isolated task reaches lastStatus=STOPPED with stopCode='TaskFailedToStart' and a stoppedReason naming CannotPullContainerError, the target group reports no healthy target, and the twin task in a public subnet reaches RUNNING (observed in ~30 s).
Actual (emulator): the check is terminated by the harness with `Check timed out after 60.0s`; neither the isolated task's terminal stop state nor the public twin's RUNNING state nor any target-health entry is ever observed.

## Steps to Reproduce
1. Create a VPC with isolated subnets (no IGW/NAT, no ECR/S3 endpoints) and a public subnet.
2. Create an ALB + target group and an ECS service (awsvpc/FARGATE, rolling controller) in the isolated subnets with desiredCount=0.
3. UpdateService desiredCount=1 and poll DescribeTasks and DescribeTargetHealth.
4. Run the identical task definition in the public subnet as a control.
5. Neither path settles: no STOPPED/TaskFailedToStart, no RUNNING control, no target health — the check times out at 60s.
