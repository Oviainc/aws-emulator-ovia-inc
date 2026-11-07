Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.asg-deregisters-terminated-instance-from-target-group autoscaling__ec2__elbv2.asg-waits-for-elb-healthy-before-marking-instance-inservice autoscaling__ec2__elbv2.cli-describe-target-health-reflects-asg-instances autoscaling__ec2__elbv2.cli-detach-instance-from-asg-decrements-target-group autoscaling__ec2__elbv2.asg-elb-health-check-triggers-instance-replacement autoscaling__ec2__elbv2.elb-target-health-change-event-not-delivered-without-eventbridge-rule
Issue-Key: autoscaling__ec2__elbv2.asg-deregisters-terminated-instance-from-target-group|_poll_until#10|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s (last observed: {last!r})")
Issue-Key: autoscaling__ec2__elbv2.asg-waits-for-elb-healthy-before-marking-instance-inservice|_poll_until#19|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s (last observed: {last!r})")
Issue-Key: autoscaling__ec2__elbv2.cli-describe-target-health-reflects-asg-instances|_poll_until#12|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last value: {last}")
Issue-Key: autoscaling__ec2__elbv2.cli-detach-instance-from-asg-decrements-target-group|_poll_until#13|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last value: {last}")
Issue-Key: autoscaling__ec2__elbv2.asg-elb-health-check-triggers-instance-replacement|run#16|Timeout|instances = asg.describe_auto_scaling_groups(AutoScalingGroupNames=[asg_name])["AutoScalingGroups"][0]["Instances"]
Issue-Key: autoscaling__ec2__elbv2.elb-target-health-change-event-not-delivered-without-eventbridge-rule|run#25|AssertionError|raise AssertionError(
Issue-Commit: edd8d27229a8bc99e308a4b32d393f13d6f32cb7

# Auto Scaling/ELBv2: ASG instances are never registered into an attached target group, and CloudWatch alarm state changes reach no EventBridge rule (2 net-new causes)

> _Note: this issue was identified against the code base at commit `edd8d27229a8bc99e308a4b32d393f13d6f32cb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAutoScalingGroup

## Description
Two distinct net-new root causes, both observed under the suite heading `autoscaling__ec2__elbv2`. The open issue 20261107T075601Z-autoscaling-ec2-elbv2-auto-scaling-elbv2-asg-instances-never-register-with.md describes a neighbouring symptom but does not carry these replay keys, so they are filed here.

Cause 1 of 2: an ASG created with TargetGroupARNs never registers its instances with the target group, so no ELB health ever exists and no ELB->ASG health propagation or deregistration can be observed.
AWS: CreateAutoScalingGroup/UpdateAutoScalingGroup accept TargetGroupARNs; Auto Scaling registers every instance that reaches the InService lifecycle state as a target of each attached group, and deregisters it when the instance is terminated or detached (detach-instances deregisters exactly the detached instance). ELBv2 DescribeTargetHealth then returns one TargetHealthDescription per registered instance whose TargetHealth.State walks the modelled enum (Smithy elasticloadbalancingv2 TargetHealthStateEnum = initial|healthy|unhealthy|unhealthy.draining|unused|draining|unavailable) from `initial` (Reason Elb.RegistrationInProgress / Elb.InitialHealthChecking) to `healthy` once the health-check target answers. With HealthCheckType=ELB an instance the load balancer reports `unhealthy` is reflected in the ASG's own per-instance HealthStatus=Unhealthy, which is the replacement trigger.
Emulator: DescribeTargetHealth on the attached group returns an empty TargetHealthDescriptions list for the whole poll window (`last observed: []`, `last value: set()`), or a description whose state is `unused` with Reason `Target.NotRegistered` and Description `Target is not registered to the target group` for an instance the ASG reports as InService. Consequently the ASG never marks the instance HealthStatus=Unhealthy for an ELB-unhealthy target, and the deregistration-on-terminate / scoped-deregistration-on-detach effects are unobservable. One check in this cause exhausted the suite's 180s budget while waiting on the same binding.
Why: a client that wires an ASG to a target group (the standard ALB/NLB autoscaling topology, by SDK, CLI or CloudFormation) sees traffic routed to nothing: DescribeTargetHealth reports no targets, health-based replacement never fires, and detach/terminate deregistration cannot be verified.
Fix: the Auto Scaling group's instance-launch/terminate state transitions must drive ELBv2 target registration/deregistration for every ARN in TargetGroupARNs, and the ELB health result must be folded into the per-instance HealthStatus when HealthCheckType=ELB (Auto Scaling group state plus its ELBv2 registration side-effect).
Traces: autoscaling__ec2__elbv2.asg-deregisters-terminated-instance-from-target-group, autoscaling__ec2__elbv2.asg-waits-for-elb-healthy-before-marking-instance-inservice, autoscaling__ec2__elbv2.cli-describe-target-health-reflects-asg-instances, autoscaling__ec2__elbv2.cli-detach-instance-from-asg-decrements-target-group, autoscaling__ec2__elbv2.asg-elb-health-check-triggers-instance-replacement.

Cause 2 of 2: a CloudWatch alarm state transition is published to no EventBridge rule, so an SQS target of a matching rule receives nothing.
AWS: every alarm state transition (including one forced with SetAlarmState) is published to the account's default event bus as an event with source `aws.cloudwatch`, detail-type `CloudWatch Alarm State Change`, and a detail carrying alarmName, state.value and previousState.value; a rule whose pattern matches those fields delivers the event to its target (here an SQS queue).
Emulator: after a rule with that exact pattern and an SQS target is created, repeated ALARM->OK transitions of the same alarm deliver no message to the queue for 120s; the check fails with `expected message for alarm <alarmName> never received on <queueUrl> despite retrying the OK transition for 120s`. The negative arm (no rule) passes only because nothing is ever delivered.
Why: alarm-driven automation built on EventBridge (notification fan-out, remediation Lambdas, queued incident records) never fires, and the emulator cannot distinguish a correctly-filtered rule from a missing one.
Fix: the CloudWatch alarm state-transition path must emit the `CloudWatch Alarm State Change` event onto the default event bus so EventBridge rule matching and target delivery apply (alarm state handler plus its event emission).
Traces: autoscaling__ec2__elbv2.elb-target-health-change-event-not-delivered-without-eventbridge-rule.

## Evidence
Cause 1 — expected: DescribeTargetHealth lists the ASG's InService instance id with TargetHealth.State converging to `healthy`; actual: `AssertionError: condition not met within 60.0s (last observed: [])`, `AssertionError: condition not met within 120.0s; last value: set()`, and `[{'Target': {'Id': '<instance-id>'}, 'HealthCheckPort': '22', 'TargetHealth': {'State': 'unused', 'Reason': 'Target.NotRegistered', 'Description': 'Target is not registered to the target group'}}]` for an instance the ASG reports InService. The ELB-health arm never saw HealthStatus=Unhealthy (its check hit `Check timed out after 180.0s`).

Cause 2 — expected: an SQS message whose body has detail-type `CloudWatch Alarm State Change`, detail.state.value `OK` and detail.previousState.value `ALARM`; actual: no matching message in 120s of retried transitions — `AssertionError: expected message for alarm sim2real-tg-alarm-<id> never received on <queue-url> despite retrying the OK transition for 120s` (alarm name, queue URL and ids are per-run values; the requirement is the event source/detail-type/detail fields).

## Steps to Reproduce
1. Replay the behavioral suite (run_behavioral_suite) and look under `### autoscaling__ec2__elbv2`.
2. Cause 1: create a launch template and an ELBv2 target group in the same VPC, create an ASG with TargetGroupARNs=[tg] and desired capacity >=1, wait for an InService instance, then call elbv2 DescribeTargetHealth(TargetGroupArn=tg) — the instance is absent (or `unused`/`Target.NotRegistered`). Then set DesiredCapacity=0 (or autoscaling detach-instances --should-decrement-desired-capacity) and observe that the registration/deregistration transition never appears. With HealthCheckType=ELB and a health check on a dead port, poll DescribeAutoScalingGroups — the instance's HealthStatus never becomes Unhealthy.
3. Cause 2: put a metric alarm, SetAlarmState ALARM, create an EventBridge rule with pattern {source:[aws.cloudwatch], detail-type:[CloudWatch Alarm State Change], detail:{alarmName:[<name>]}} and an SQS target, then SetAlarmState OK and poll the queue — no message arrives.
