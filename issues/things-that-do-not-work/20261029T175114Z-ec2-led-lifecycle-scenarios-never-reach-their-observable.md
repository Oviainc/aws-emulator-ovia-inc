Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.interruptible-capacity-reservation-allocation-lifecycle autoscaling__ec2__elbv2.asg-scale-in-respects-termination-policy autoscaling__ec2__elbv2.elb-target-health-change-event-not-delivered-without-eventbridge-rule cloudwatch__ec2.xsvc-cloudwatch-3-alarm-ec2-terminate-action-disabled-blocked-until-enabled ec2__ssm.nacls-are-stateless-but-security-groups-are-stateful
Issue-Key: ec2.interruptible-capacity-reservation-allocation-lifecycle|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: autoscaling__ec2__elbv2.asg-scale-in-respects-termination-policy|run#18|Timeout|time.sleep(15)
Issue-Key: autoscaling__ec2__elbv2.elb-target-health-change-event-not-delivered-without-eventbridge-rule|_receive_matching_message#24|Timeout|resp = sqs.receive_message(
Issue-Key: cloudwatch__ec2.xsvc-cloudwatch-3-alarm-ec2-terminate-action-disabled-blocked-until-enabled|_wait_for#27|Timeout|time.sleep(interval_s)
Issue-Key: ec2__ssm.nacls-are-stateless-but-security-groups-are-stateful|_poll#5|Timeout|time.sleep(min(interval_s, max(0.0, deadline - time.monotonic())))
Issue-Commit: abd75e982d55cd9e91541cb3d125feb2d8edb60f

# EC2-led lifecycle scenarios never reach their observable terminal state, so five checks exhaust their 60s budget instead of converging (1 cause)

> _Note: this issue was identified against the code base at commit `abd75e982d55cd9e91541cb3d125feb2d8edb60f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceStatus

## Description
Cause 1 of 1: five lifecycle scenarios that poll an EC2-owned observable condition never see it become true and are cut off by the harness after 60 seconds.

AWS: recorded behavior only. Each of these scenarios advances through EC2 states that AWS reaches within seconds and then exposes on a read: an interruptible capacity-reservation allocation moves to its allocated/active state; a scaled-in ASG member moves to shutting-down then terminated so DescribeAutoScalingGroups/DescribeInstances settle; a registered target's health transitions so the ELB target-health change is observable; an EC2 instance acted on by a CloudWatch alarm action moves to terminated once the alarm is enabled; and a security group's stateful return traffic is admitted while a NACL's is not, which the scenario observes from inside the guest.
Emulator: the polled condition never becomes true. The replay reports exactly `Error: Check timed out after 60.0s` with no exception from the emulator, i.e. the calls answered but the state they report never advanced to the terminal value.
Why: a client that waits on an EC2 state transition (every waiter in every SDK, every CloudFormation/CDK deploy, every teardown script) blocks forever instead of completing. This is strictly worse than an error: there is no code to catch.
Hypothesis: one shared state-advance step (the monotonic lifecycle clock that promotes pending -> running -> shutting-down -> terminated and the derived health/allocation states) is not driven on these paths, so reads are internally consistent but frozen. The replay output does not state which step, so the fix must start by instrumenting the lifecycle advance for the five scenarios below.
Fix: the EC2 instance/allocation lifecycle state machine and the derived views that scale-in, target health and alarm actions read (EC2 state, not any single op handler).
Traces: ec2.interruptible-capacity-reservation-allocation-lifecycle, autoscaling__ec2__elbv2.asg-scale-in-respects-termination-policy, autoscaling__ec2__elbv2.elb-target-health-change-event-not-delivered-without-eventbridge-rule, cloudwatch__ec2.xsvc-cloudwatch-3-alarm-ec2-terminate-action-disabled-blocked-until-enabled, ec2__ssm.nacls-are-stateless-but-security-groups-are-stateful.

## Evidence
All five checks report the same verdict line and nothing else: `Error: Check timed out after 60.0s`. No emulator exception, no non-2xx response — the operations answered, but the state they returned never reached the terminal value the scenario polls for. Expected: the polled condition becomes true within the scenario's budget (AWS reaches these states in seconds). Actual: the condition is still false after 60s. The 60s figure is the harness budget, not a requirement; the requirement is that the transition happens at all.

## Steps to Reproduce
1. Boot the emulator.
2. ec2.interruptible-capacity-reservation-allocation-lifecycle: create an interruptible capacity reservation allocation and poll its state; it never leaves its initial state.
3. autoscaling__ec2__elbv2.asg-scale-in-respects-termination-policy: create an ASG with two members and a termination policy, reduce DesiredCapacity by one, then poll DescribeAutoScalingGroups/DescribeInstances for the terminated member.
4. cloudwatch__ec2.xsvc-cloudwatch-3-alarm-ec2-terminate-action-disabled-blocked-until-enabled: create a disabled alarm with an EC2 terminate action, enable it, drive it to ALARM, then poll the instance state for 'terminated'.
5. Each poll loop never satisfies its predicate. Re-replay the check ids in trace_ids.
