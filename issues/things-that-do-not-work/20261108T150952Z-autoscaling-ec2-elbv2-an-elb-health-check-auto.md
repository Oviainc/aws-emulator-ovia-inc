Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.asg-elb-health-check-triggers-instance-replacement
Issue-Key: autoscaling__ec2__elbv2.asg-elb-health-check-triggers-instance-replacement|run#16|Timeout|health = elbv2.describe_target_health(TargetGroupArn=tg_arn, Targets=[{"Id": original_id}])["TargetHealthDescriptions"]
Issue-Commit: 85304fbaaec1e137da409d02b027804fcd09632b

# autoscaling__ec2__elbv2: an ELB-health-check Auto Scaling group never derives its instance's HealthStatus=Unhealthy from target health, so the ELB→ASG replacement trigger never fires

> _Note: this issue was identified against the code base at commit `85304fbaaec1e137da409d02b027804fcd09632b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAutoScalingGroups

## Description
One net-new root cause.

Cause 1 of 1: ELB→ASG health propagation never happens for HealthCheckType=ELB.
AWS: Smithy (com.amazonaws.autoscaling): AutoScalingGroup.HealthCheckType is "One or more comma-separated health check types for the Auto Scaling group" and Instance.HealthStatus is documented "The last reported health status of the instance. Healthy means that the instance is healthy and should remain in service. Unhealthy means that the instance is unhealthy and that Amazon EC2 Auto Scaling should terminate and replace it." Recorded behavior for the wiring: for a group created with HealthCheckType="ELB", a HealthCheckGracePeriod and TargetGroupARNs naming an instance-type target group whose health check points at a port nothing listens on, ELBv2 DescribeTargetHealth eventually reports that instance's target state "unhealthy", and after the grace period the Auto Scaling group's own per-instance record (DescribeAutoScalingGroups → AutoScalingGroups[0].Instances[].HealthStatus) turns to "Unhealthy" — the trigger for replacement. The recorded run of this sequence completes inside the suite's 180s budget for the check.
Emulator: the check never reaches either assertion — "Check timed out after 180.0s". Its loop polls DescribeTargetHealth and DescribeAutoScalingGroups every 2s and exits only when the instance's ASG HealthStatus reads "Unhealthy"; it never did, so the budget expired before the assertions "target health for the original instance never reported Unhealthy" / "ASG never marked the ELB-unhealthy instance's own HealthStatus as Unhealthy -- the ELB->ASG health-check wiring did not propagate" could be evaluated. Hypothesis: the ASG neither registers its launched instance into the attached target group nor consumes target health, so neither the ELBv2 side nor the ASG side ever leaves its initial state.
Why: HealthCheckType=ELB is the standard way to let a load balancer's view drive instance replacement. A client (or IaC test) that configures it gets a group that reports every instance Healthy forever, so no unhealthy instance is ever replaced and failure-recovery behaviour cannot be exercised at all.
Fix: the Auto Scaling health-check housekeeping for groups with HealthCheckType=ELB — read the attached target groups' target health (after HealthCheckGracePeriod) and set the per-instance HealthStatus to Unhealthy, together with the ASG→target-group registration the propagation depends on.
Related but not covering: the open issues 20261107T075601Z-autoscaling-ec2-elbv2-auto-scaling-elbv2-asg-instances-never-register-with.md and 20261107T173527Z-autoscaling-ec2-elbv2-auto-scaling-elbv2-asg-instances-are-never-registered.md describe ASG instances not being registered with target groups; the replay marked this check as owned by neither, so the ELB→ASG HealthStatus derivation is filed here.
Traces: autoscaling__ec2__elbv2.asg-elb-health-check-triggers-instance-replacement.

## Evidence
Replay output for autoscaling__ec2__elbv2.asg-elb-health-check-triggers-instance-replacement: `Error: Check timed out after 180.0s`. The check's own waits are bounded well above the harness budget (LB-active 190s, instance-appearance poll 300s, health-propagation loop 420s with a 2s interval) and they all exit early as soon as the condition holds, so a timeout means no condition ever held: the instance's ASG HealthStatus never became "Unhealthy" and the target was never reported "unhealthy". Other autoscaling checks in the same replay did answer, so the emulator was serving.

## Steps to Reproduce
1. Create a launch template for a t3.micro in the shared VPC and an instance-type target group in that VPC with Port=22, HealthCheckPort=55555 (nothing listens there), interval 10s, healthy/unhealthy thresholds 2.
2. CreateAutoScalingGroup with MinSize=MaxSize=DesiredCapacity=1, that launch template, TargetGroupARNs=[<tg arn>], HealthCheckType="ELB", HealthCheckGracePeriod=30.
3. Create an internal NLB in the same subnets, wait for it to become active, and create a listener forwarding to the target group.
4. Poll ELBv2 DescribeTargetHealth for the launched instance: expect State="unhealthy".
5. Poll DescribeAutoScalingGroups for the group: expect that instance's HealthStatus to turn "Unhealthy" (the replacement trigger).
