Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecs.cdk-l2-ecs-asg-capacity-provider-managed-scaling-reaches-the-ecs-api
Issue-Key: cloudformation__ecs.cdk-l2-ecs-asg-capacity-provider-managed-scaling-reaches-the-ecs-api|_teardown#15|Timeout|time.sleep(10.0)
Issue-Commit: 92bcdac01f88c8b4612f5537a2ed608090c91758

# cloudformation__ecs: the CDK L2 ECS/ASG capacity-provider scenario produces no verdict — the deploy-and-register path never settles and the check is killed at the harness cap (1 cause)

> _Note: this issue was identified against the code base at commit `92bcdac01f88c8b4612f5537a2ed608090c91758`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause, grounded in one failing behavioral check. No open issue carries this check's replay key, and the suite marks it neither ALREADY FILED nor OWNED.

Cause 1 of 1 — a CDK L2 app that creates a VPC, an ASG with a launch template, an ECS cluster and an AsgCapacityProvider with enable_managed_scaling=True never reaches a measurable state: the check is cut off by the harness's 60s cap with no assertion result, so none of the capacity-provider reads, the ECS-created scaling policy, or the container-instance registration are ever answered.
AWS: recorded behavior only (the scenario's reference is https://docs.aws.amazon.com/AmazonECS/latest/developerguide/asg-capacity-providers.html). `cdk bootstrap` then `cdk deploy` of this app succeeds and the stack's resources include an AWS::EC2::LaunchTemplate and NO AWS::AutoScaling::LaunchConfiguration and NO AWS::AutoScaling::ScalingPolicy (the app pins the generateLaunchTemplateInsteadOfLaunchConfig context flag and declares no policy). DescribeCapacityProviders([<provider>]) then returns exactly one provider with status ACTIVE whose autoScalingGroupProvider names the stack's ASG, managedDraining ENABLED, managedTerminationProtection DISABLED and managedScaling.status ENABLED with an integer targetCapacity in 1..100. Because managed scaling is ENABLED, ECS itself creates exactly ONE scaling policy on that ASG out of band — DescribePolicies(AutoScalingGroupName) returns one policy of PolicyType TargetTrackingScaling whose name is none of the stack's logical/physical ids, tracking the customized metric AWS/ECS/ManagedScaling / CapacityProviderReservation — and a container instance launched by the ASG registers with the cluster and reads back ACTIVE.
Emulator: no observable outcome at all. The replay verdict is only `Check timed out after 60.0s`, so the emulator produced neither a converged stack nor any of the ECS/Auto Scaling reads above.
Hypothesis (where it stalls): the check's phase bounds are bootstrap 300s, cdk deploy 600s, container-instance registration 420s, teardown 420s — all far above the 60s cap, so the stall is in one of them. Other CDK-driven checks in this same suite do get past `cdk bootstrap`/`cdk deploy` and fail on assertions within the cap, which makes a CLI/runner budget artifact unlikely and points at the deploy of this resource set (VPC/NAT + ASG + launch template + ECS cluster + capacity provider) never converging, or the container-instance registration poll never observing an ACTIVE instance because nothing registers one.
Why: the documented ECS capacity-provider workflow (CDK/CFN-declared ASG capacity provider with managed scaling, then ECS placing capacity on it) yields no status a client can branch on: no terminal stack state, no ACTIVE provider, no ECS-created target-tracking policy, no registered container instance — and a hang is indistinguishable from an unsupported resource type.
Fix: the AWS::ECS::CapacityProvider / AWS::ECS::ClusterCapacityProviderAssociations and AWS::AutoScaling::AutoScalingGroup + AWS::EC2::LaunchTemplate provisioners in the cloudformation crate's resource-provisioner layer must always drive the stack to a terminal state (never leave the deploy unresolved); the ecs crate's capacity-provider state must report status ACTIVE with the autoScalingGroupProvider fields the request set (managedScaling, managedDraining, managedTerminationProtection), create the managed-scaling target-tracking policy on the named ASG out of band in the autoscaling crate's state (customized metric AWS/ECS/ManagedScaling / CapacityProviderReservation), and register an ASG-launched instance as an ACTIVE container instance of the cluster.
Traces: cloudformation__ecs.cdk-l2-ecs-asg-capacity-provider-managed-scaling-reaches-the-ecs-api

## Evidence
Failing check cloudformation__ecs.cdk-l2-ecs-asg-capacity-provider-managed-scaling-reaches-the-ecs-api — verdict [mismatch], detail: `Error: Check timed out after 60.0s` (no assertion reached).

Expected (AWS), in order:
  1. `cdk bootstrap` (<=300s) and `cdk deploy --require-approval never --outputs-file out.json` (<=600s) succeed; outputs carry VpcId, ClusterName, CapacityProviderName, AsgName.
  2. The stack's resource types include AWS::EC2::LaunchTemplate and exclude AWS::AutoScaling::LaunchConfiguration and AWS::AutoScaling::ScalingPolicy.
  3. DescribeCapacityProviders(capacityProviders=[<provider>]) -> exactly 1, status 'ACTIVE', autoScalingGroupProvider.autoScalingGroupArn ending in '/<AsgName>', managedDraining 'ENABLED', managedTerminationProtection 'DISABLED', managedScaling.status 'ENABLED', managedScaling.targetCapacity an int in 1..100.
  4. DescribePolicies(AutoScalingGroupName=<AsgName>) -> exactly 1 policy, PolicyType 'TargetTrackingScaling', PolicyName not among the stack's logical/physical ids, tracking AWS/ECS/ManagedScaling / CapacityProviderReservation.
  5. A container instance from the ASG registers with the cluster and reads back ACTIVE (<=420s).

Actual (emulator): none of the above was reached; the check was cut off at the 60s harness cap, so no stack status, capacity provider, scaling policy or container instance was observed. Comparable CDK-driven checks in this suite (e.g. cloudformation.cdk-deployed-l2-trio-serves-http-and-persists, cloudformation__ec2.cdk-diff-sees-a-template-change-and-is-blind-to-resource-drift) do complete their cdk invocations inside the same cap and fail on assertions, so the cap alone does not explain this verdict.

## Steps to Reproduce
1. Boot the emulator. Stage the CDK app (max_azs=2 VPC, ASG with a launch template via the generateLaunchTemplateInsteadOfLaunchConfig context flag, ECS cluster, AsgCapacityProvider with enable_managed_scaling=True, enable_managed_draining=True, enable_managed_termination_protection=False) and run `cdk bootstrap` then `cdk deploy --require-approval never --outputs-file out.json`.
2. Observe whether the deploy converges to a terminal CloudFormation state at all; if it does, read the outputs (VpcId, ClusterName, CapacityProviderName, AsgName) and ListStackResources.
3. Assert the stack carries an AWS::EC2::LaunchTemplate and no AWS::AutoScaling::LaunchConfiguration and no AWS::AutoScaling::ScalingPolicy.
4. ecs.DescribeCapacityProviders(capacityProviders=[<CapacityProviderName>]) -> expect 1 provider, status ACTIVE, autoScalingGroupProvider pointing at <AsgName>, managedDraining ENABLED, managedTerminationProtection DISABLED, managedScaling.status ENABLED, managedScaling.targetCapacity in 1..100.
5. autoscaling.DescribePolicies(AutoScalingGroupName=<AsgName>) -> expect exactly one ECS-created TargetTrackingScaling policy, named outside the stack's ids, tracking AWS/ECS/ManagedScaling / CapacityProviderReservation.
6. Poll ecs.ListContainerInstances/DescribeContainerInstances on <ClusterName> -> expect an ASG-launched instance to register and read ACTIVE.
