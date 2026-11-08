Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.deploy-cfn-asg-scaling-policy-with-target-group
Issue-Key: autoscaling__ec2__elbv2.deploy-cfn-asg-scaling-policy-with-target-group|_in_service_ids#9|Timeout|return [
Issue-Commit: f6c480f97b4e9ab6af0fc97739f0c916ef94d26b

# A CloudFormation-deployed ASG + ScalingPolicy + target-group stack never converges and its policy-driven scale-out is unreachable (ExecutePolicy answers 501)

> _Note: this issue was identified against the code base at commit `f6c480f97b4e9ab6af0fc97739f0c916ef94d26b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecutePolicy

## Description
Cause 1 of 1: the end-to-end flow 'CloudFormation stack declaring a launch-template ASG wired to an ELBv2 target group plus an AWS::AutoScaling::ScalingPolicy, then a forced policy execution that scales the group out' makes no progress against the emulator. The check exhausts its wall-clock budget with nothing converged ('Check timed out after 180.0s', no response frame), and the scale-out leg it is built around cannot succeed at all because ExecutePolicy is an unimplemented scaffold that answers HTTP 501 NotImplemented.

AWS: the stack (AWS::EC2::VPC + two Subnets, SecurityGroup with tcp/80 ingress, AWS::EC2::LaunchTemplate, AWS::ElasticLoadBalancingV2::TargetGroup Port 80/Protocol HTTP/TargetType instance, an internal AWS::ElasticLoadBalancingV2::LoadBalancer + Listener forwarding to it, AWS::AutoScaling::AutoScalingGroup with MinSize 1 / MaxSize 3 / DesiredCapacity 1, TargetGroupARNs [!Ref TargetGroup], DependsOn Listener, and AWS::AutoScaling::ScalingPolicy PolicyType SimpleScaling / AdjustmentType ChangeInCapacity / ScalingAdjustment 1 / Cooldown 0) reaches CREATE_COMPLETE and exports TargetGroupArn, AutoScalingGroupName and ScalingPolicyArn. DescribeAutoScalingGroups then reports exactly one AutoScalingGroups[0].Instances entry with LifecycleState 'InService'. ExecutePolicy(PolicyName=<policy name or ARN>, HonorCooldown=false) returns 200 with an empty body and applies the ChangeInCapacity +1 adjustment, so the group raises DesiredCapacity to 2 and launches a second instance that also reaches 'InService'; elbv2 DescribeTargetHealth(TargetGroupArn=<tg>) then lists the new instance id under TargetHealthDescriptions[].Target.Id with TargetHealth.State moving initial -> healthy. Smithy (autoscaling 2011-01-01): ExecutePolicy input PolicyName (ResourceName, len 1..1600) required — 'The name or ARN of the policy'; HonorCooldown (valid only for SimpleScaling); modeled errors ScalingActivityInProgressFault (HTTP 400) and ResourceContentionFault (HTTP 500) — 501/NotImplemented is NOT a modeled outcome. The convergence itself (InService, registration, healthy) is recorded behavior only; the models state the representation, not the timing.

Emulator: nothing in that chain converges. Replay reports 'Error: Check timed out after 180.0s' with no response frame — the emulator did not answer one call wrongly, it never reached the state the flow waits for (terminal stack status with the three Outputs, then a baseline InService instance). In addition, the operation the check is built on, ExecutePolicy, is a scaffold that unconditionally returns the scaffold error — HTTP 501, error code NotImplemented, message 'operation ExecutePolicy is scaffolded but not implemented in this emulator' (fixed text, with the operation name as its only variable part) — so even with a converged baseline the policy-driven scale-out and the subsequent target registration can never be observed.

Why: this is the canonical template-driven deployment of a load-balanced, auto-scaled service plus the standard way scaling is exercised deterministically in tests and runbooks (`aws autoscaling execute-policy --no-honor-cooldown`). A client that deploys the stack and waits for the baseline instance to be InService hangs with no way to distinguish 'still launching' from 'will never launch'; a client that then executes the policy gets a 501 its SDK models no error for, instead of a 200 and a +1 capacity change, so scale-out testing, CFN-declared ScalingPolicy resources and every downstream assertion (new instance registered as a target, target healthy, listener forwarding to it) are unreachable.

Fix: the ScalingPolicy execution handler services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/execute_policy.rs (resolve PolicyName as a name or ARN against the group's policies, apply the adjustment to DesiredCapacity, honour/skip the cooldown per HonorCooldown, refuse with ScalingActivityInProgressFault when a scaling activity is in flight), together with the Auto Scaling group reconciliation that turns DesiredCapacity into EC2 instances and advances their LifecycleState, and its propagation into ELBv2 target-group registration and health feedback as reached through the CloudFormation provisioners for AWS::AutoScaling::AutoScalingGroup (incl. TargetGroupARNs) and AWS::AutoScaling::ScalingPolicy — the group/instance state and the registration path, not the describe serializers.

Related open issues that describe the same family but do NOT carry this check's replay key (the suite did not mark it owned): 20261108T052358Z-autoscaling-ec2-elbv2-a-cloudformation-deployed-asg-alb-target-group-stack.md (same non-convergence without the ScalingPolicy/ExecutePolicy leg), 20261107T075601Z-autoscaling-ec2-elbv2-auto-scaling-elbv2-asg-instances-never-register-with.md and 20261024T015148Z-autoscaling-auto-scaling-putscalingpolicy-is-an-unimplemented-scaffold-stub.md (PutScalingPolicy, a different operation and file).

Traces: autoscaling__ec2__elbv2.deploy-cfn-asg-scaling-policy-with-target-group.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (62/414 passed; the emulator is up and answering — other checks in this service returned real responses).

[mismatch] autoscaling__ec2__elbv2.deploy-cfn-asg-scaling-policy-with-target-group:
  Error: Check timed out after 180.0s

The check's waits, in order: (1) DescribeStacks until the stack status is in {CREATE_COMPLETE, CREATE_FAILED, ROLLBACK_COMPLETE, ROLLBACK_FAILED} (400s) and then an assertion that it is CREATE_COMPLETE with Outputs TargetGroupArn / AutoScalingGroupName / ScalingPolicyArn; (2) describe_auto_scaling_groups(AutoScalingGroupNames=[asg]) until some Instances[].LifecycleState == 'InService' (400s, 'baseline ASG instance reaches InService'); (3) execute_policy(PolicyName=<ScalingPolicyArn>, HonorCooldown=False); (4) the same describe until >= 2 InService instances (400s, 'ASG scales out to 2 InService instances'); (5) describe_target_health(TargetGroupArn=tg) until the newly launched instance ids are healthy (300s). None of them completed within the harness budget.

Expected (recording): stack CREATE_COMPLETE with the three Outputs; exactly one baseline instance with LifecycleState 'InService'; ExecutePolicy -> HTTP 200, empty body; a second instance reaching 'InService'; DescribeTargetHealth listing that new instance id with TargetHealth.State 'healthy'.

Actual: no InService instance is ever observed, so the check is cut off by its budget. Independently, services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/execute_policy.rs:188 returns `Scaffolded::not_implemented("ExecutePolicy")` for every request — HTTP 501, code NotImplemented, message 'operation ExecutePolicy is scaffolded but not implemented in this emulator' — where AWS answers 200 and applies the adjustment; the file still carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker at line 4.

(The 180s budget, the stack/group/policy names, instance ids and timestamps are per-run values; the requirements are the statuses, the error-free 200 from ExecutePolicy, the +1 capacity change, and prompt convergence of the group and its target registrations.)

## Steps to Reproduce
1. Boot the emulator and resolve an AMI id (ssm get-parameter /aws/service/ami-amazon-linux-latest/amzn2-ami-hvm-x86_64-gp2).
2. CreateStack with a template containing: AWS::EC2::VPC (10.40.0.0/16) + two Subnets in different AZs, AWS::EC2::SecurityGroup with tcp/80 ingress from 10.40.0.0/16, AWS::EC2::LaunchTemplate (that ImageId, t3.micro, that SG, UserData starting `python3 -m http.server 80`), AWS::ElasticLoadBalancingV2::TargetGroup (VpcId, Port 80, Protocol HTTP, TargetType instance), an internal AWS::ElasticLoadBalancingV2::LoadBalancer + Listener forwarding to it, AWS::AutoScaling::AutoScalingGroup (MinSize '1', MaxSize '3', DesiredCapacity '1', VPCZoneIdentifier the two subnets, LaunchTemplate with Version !GetAtt LatestVersionNumber, TargetGroupARNs [!Ref TargetGroup], DependsOn Listener), and AWS::AutoScaling::ScalingPolicy (SimpleScaling, ChangeInCapacity, ScalingAdjustment 1, Cooldown '0'). Outputs: TargetGroupArn, AutoScalingGroupName, ScalingPolicyArn.
3. Poll DescribeStacks until terminal; expect CREATE_COMPLETE with all three Outputs.
4. Poll autoscaling DescribeAutoScalingGroups(AutoScalingGroupNames=[<asg>]) for an instance with LifecycleState 'InService'. Observe it never appears.
5. Call autoscaling ExecutePolicy(PolicyName=<ScalingPolicyArn>, HonorCooldown=false). Observe HTTP 501 NotImplemented ('operation ExecutePolicy is scaffolded but not implemented in this emulator') instead of a 200 and a DesiredCapacity increase to 2.
6. Poll DescribeAutoScalingGroups for a second InService instance and elbv2 DescribeTargetHealth(TargetGroupArn=<tg>) for that instance becoming 'healthy'. Observe neither happens.
7. Re-replay autoscaling__ec2__elbv2.deploy-cfn-asg-scaling-policy-with-target-group to confirm a fix.
