Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.deploy-cfn-stack-asg-elb-ec2-integration
Issue-Key: autoscaling__ec2__elbv2.deploy-cfn-stack-asg-elb-ec2-integration|_poll_until#9|Timeout|last = probe()
Issue-Commit: 8c5329059e752b9fa65691305eadc8c02e100ead

# A CloudFormation-deployed ASG + ALB + target-group stack never converges: the launch-template ASG's instance is never InService and never registers/becomes healthy in the target group, so the whole flow exhausts its budget

> _Note: this issue was identified against the code base at commit `8c5329059e752b9fa65691305eadc8c02e100ead`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAutoScalingGroup

## Description
Cause 1 of 1: the end-to-end CloudFormation flow 'stack with a launch-template ASG wired to an ELBv2 target group' never reaches the state the recording reaches — the stack's ASG instance is never reported InService, is never registered as a target, and never becomes healthy — so the check makes no progress at all and ends on its wall-clock budget with nothing converged.

AWS: a stack declaring AWS::EC2::SecurityGroup (+SecurityGroupIngress), AWS::EC2::LaunchTemplate, AWS::ElasticLoadBalancingV2::TargetGroup (Port 80, Protocol HTTP, TargetType instance, HealthCheckIntervalSeconds 10, HealthyThresholdCount 2), AWS::ElasticLoadBalancingV2::LoadBalancer (internal ALB) + Listener, and AWS::AutoScaling::AutoScalingGroup (MinSize/MaxSize/DesiredCapacity 1, TargetGroupARNs: [!Ref TargetGroup], DependsOn: Listener) reaches CREATE_COMPLETE with the declared Outputs, and then: autoscaling DescribeAutoScalingGroups reports AutoScalingGroups[0].Instances with an entry whose LifecycleState is 'InService'; elbv2 DescribeTargetHealth(TargetGroupArn=<tg>) lists exactly that instance id under TargetHealthDescriptions[].Target.Id and its TargetHealth.State moves initial -> healthy. Recorded behavior only — the Smithy models (autoscaling Instance.LifecycleState, elbv2 TargetHealth.State) state the representation, not the convergence.

Emulator: nothing in that chain converges. The check never gets past its waits and the replay reports 'Error: Check timed out after 180.0s' with no response frame — i.e. the emulator did not answer wrongly at one call, it simply never reached the state the flow waits for (ASG instance InService, then registered in the target group, then healthy). Every sibling check in this same family fails the same way (they are already filed, see below), so the non-convergence is systemic rather than specific to this template.

Why: this is the canonical template-driven deployment of a load-balanced, auto-scaled service. A client that deploys it and then waits for the ASG's instance to be InService and for its targets to be healthy — the standard CI/CD deploy gate, and what CloudFormation resource signals, `aws autoscaling wait` and CDK deploys do — hangs forever and cannot distinguish 'still launching' from 'will never launch'. Everything downstream (listener routing to the instance, scale-out adding targets, scale-in draining them) is unreachable.

Fix: the Auto Scaling group reconciliation that turns DesiredCapacity into EC2 instances and advances their LifecycleState, together with its propagation into ELBv2 target-group registration and health feedback, as reached through the CloudFormation provisioners for AWS::AutoScaling::AutoScalingGroup (incl. TargetGroupARNs) — the group/instance state and the registration path, not the describe serializers.

Related open issue that describes the same family but does NOT carry this check's current replay key (its key for this check is a `_poll_until#9` Timeout frame; this replay produces a bare 180s wall-clock timeout with no frame, so the suite did not mark it owned): 20261107T075601Z-autoscaling-ec2-elbv2-auto-scaling-elbv2-asg-instances-never-register-with.md (Cause 1). Also neighbouring: 20261107T173527Z-autoscaling-ec2-elbv2-auto-scaling-elbv2-asg-instances-are-never-registered.md.

Traces: autoscaling__ec2__elbv2.deploy-cfn-stack-asg-elb-ec2-integration.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (50/414 passed; the emulator is up and answering — other checks in this service returned real responses).

[mismatch] autoscaling__ec2__elbv2.deploy-cfn-stack-asg-elb-ec2-integration:
  Error: Check timed out after 180.0s

The check's own waits, in order, are: CFN stack status in {CREATE_COMPLETE, CREATE_FAILED, ROLLBACK_COMPLETE, ROLLBACK_FAILED} (400s); then `describe_auto_scaling_groups(AutoScalingGroupNames=[asg])` until some Instances[].LifecycleState == 'InService' (400s, description 'ASG instance reaches InService'); then `describe_target_health(TargetGroupArn=tg)` until the InService instance ids are a subset of TargetHealthDescriptions[].Target.Id (180s, 'EC2 instances register with the target group'); then until those ids all have TargetHealth.State == 'healthy' (300s). None of them completed.

Expected (recording): the stack reaches CREATE_COMPLETE and exports TargetGroupArn/AutoScalingGroupName, the ASG's single instance reaches LifecycleState 'InService', DescribeTargetHealth lists exactly that instance id, and its TargetHealth.State becomes 'healthy'.

Actual: no InService instance and no registered/healthy target are ever observed; the check is cut off by its budget. (The 180s budget, stack/group names and instance ids are per-run values; the requirement is that the emulated stack converges promptly, as the sibling checks in this family — all timing out identically — also show.)

## Steps to Reproduce
1. Boot the emulator and resolve an AMI id (ssm get-parameter /aws/service/ami-amazon-linux-latest/amzn2-ami-hvm-x86_64-gp2) plus a VPC with two subnets.
2. CreateStack with a template containing: AWS::EC2::SecurityGroup + SecurityGroupIngress (tcp 80 from itself), AWS::EC2::LaunchTemplate (that ImageId, t3.micro, the SG, UserData starting an HTTP server on :80), AWS::ElasticLoadBalancingV2::TargetGroup (VpcId, Port 80, Protocol HTTP, TargetType instance), an internal AWS::ElasticLoadBalancingV2::LoadBalancer + Listener forwarding to it, and AWS::AutoScaling::AutoScalingGroup (MinSize/MaxSize/DesiredCapacity '1', VPCZoneIdentifier the subnets, LaunchTemplate with Version !GetAtt LatestVersionNumber, TargetGroupARNs [!Ref TargetGroup], DependsOn Listener). Outputs: TargetGroupArn, AutoScalingGroupName.
3. Poll DescribeStacks until the status is terminal; expect CREATE_COMPLETE with both Outputs.
4. Poll autoscaling DescribeAutoScalingGroups(AutoScalingGroupNames=[<asg>]) for an instance with LifecycleState 'InService'. Observe it never appears.
5. Poll elbv2 DescribeTargetHealth(TargetGroupArn=<tg>) for that instance id and then for TargetHealth.State == 'healthy'. Observe neither happens.
6. Re-replay autoscaling__ec2__elbv2.deploy-cfn-stack-asg-elb-ec2-integration to confirm a fix.
