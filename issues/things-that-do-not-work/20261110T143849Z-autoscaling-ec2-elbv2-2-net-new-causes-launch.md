Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.deploy-cfn-asg-launch-template-ec2-instances autoscaling__ec2__elbv2.e2e-asghc-asg-instance-health-check-failure-lifecycle
Issue-Key: autoscaling__ec2__elbv2.deploy-cfn-asg-launch-template-ec2-instances|run#14|AssertionError|assert tags.get("sim2real:template-check") == "v1", (
Issue-Key: autoscaling__ec2__elbv2|DescribeLoadBalancers|LoadBalancerNotFound
Issue-Commit: 8ffd4f499c6d6f6b1d86ee4455070f9416ebee70

# autoscaling__ec2__elbv2: 2 net-new causes — launch-template instance TagSpecifications are not applied to an ASG-launched instance, and a CREATE_COMPLETE stack's NLB ARN is unknown to elbv2

> _Note: this issue was identified against the code base at commit `8ffd4f499c6d6f6b1d86ee4455070f9416ebee70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstances

## Description
Two net-new root causes in the CloudFormation -> Auto Scaling -> EC2/ELBv2 path. Both are grounded in failing behavioral checks of this suite. Neither is covered by an existing open issue; the nearest neighbours are named per cause.

Cause 1 of 2: tags declared in a launch template's LaunchTemplateData.TagSpecifications (ResourceType: instance) are absent from the instance the Auto Scaling group launches.
AWS: Smithy (EC2) — LaunchTemplateTagSpecificationRequest: "The tags specification for the resources that are created during instance launch"; ResourceType may be instance | volume | network-interface | spot-instances-request, and "The tags to apply to the resource". So an instance launched from a launch-template version that carries TagSpecifications{ResourceType=instance, Tags=[...]} is created already carrying every one of those key/value pairs, and DescribeInstances reports them in Instances[].Tags — whether the launch came from RunInstances or from an Auto Scaling group using that LaunchTemplate. The ASG's own Tags with PropagateAtLaunch=true are applied on top of them (a separate mechanism); AWS returns the union.
Emulator: after the stack reaches CREATE_COMPLETE and the group's instance reaches InService, DescribeInstances on that instance id returns Tags with no entry for the launch template's instance tag at all: the check reads `None` where the template declares the value, i.e. the member is missing from the Tags list rather than carrying a wrong value. The assertion that fails is `expected propagated tag <key>=<value>, got None` (the key and value here come from the template under replay, not from this one run).
Why: a client that selects, filters or bills instances by launch-template tags (DescribeInstances Filters=[{Name: tag:<key>}], cost allocation, cleanup sweeps by a managed-by tag) sees an untagged fleet. Sweeps that delete only tagged resources leave the instance behind, and policies conditioned on aws:ResourceTag never match.
Fix: the instance-creation path that materialises a launch-template version — the launch-template resolution used by RunInstances and by the Auto Scaling group's launch (EC2 RunInstances handler / launch-template version expansion and the instance's tag state). The launch-template version's TagSpecifications for ResourceType=instance must be merged into the new instance's tag set so DescribeInstances serves them; volume-scoped specs likewise belong on the created volume.
Traces: autoscaling__ec2__elbv2.deploy-cfn-asg-launch-template-ec2-instances.

Cause 2 of 2: a CloudFormation stack that reports CREATE_COMPLETE outputs a load-balancer ARN that ELBv2 does not know, so DescribeLoadBalancers refuses it.
AWS: recorded behavior only for the stack flow — when CreateStack reaches CREATE_COMPLETE, every declared resource exists in its own service, so DescribeLoadBalancers(LoadBalancerArns=[<the stack's NLB ARN output>]) answers 200 with that load balancer and State.Code eventually 'active'. The refusal the emulator returns is AWS's wording for an ARN that genuinely does not exist: HTTP 400, error code LoadBalancerNotFound, message "One or more load balancers not found" (fixed text, no variable part) — correct for a nonexistent ARN, wrong for an ARN the stack just published.
Emulator: in the first phase of the lifecycle check, DescribeLoadBalancers on the ARN taken from the CREATE_COMPLETE stack's Outputs is refused with LoadBalancerNotFound / "One or more load balancers not found", so no later phase runs. The stack therefore reported success for an AWS::ElasticLoadBalancingV2::LoadBalancer it never registered with ELBv2 (or registered under a different ARN than the one it exported).
Why: every client that deploys infrastructure as a stack and then drives it through the service APIs breaks at the first call: the ARN CloudFormation hands out is unusable, and the failure looks like a client bug (a bad ARN) rather than a provisioning gap. Deletion and attachment flows that resolve the output ARN fail the same way.
Fix: the CloudFormation resource provisioner for AWS::ElasticLoadBalancingV2::LoadBalancer and the ELBv2 load-balancer state it must write (stack resource creation + the physical id/ARN it exports must be the one DescribeLoadBalancers resolves). Related open issue 20261023T205437Z-cloudformation-reports-create-complete-for-resources-it-never.md describes the same family for other resources but does not carry this check.
Traces: autoscaling__ec2__elbv2.e2e-asghc-asg-instance-health-check-failure-lifecycle (phase 1/6 deploy).

## Evidence
Cause 1 — check autoscaling__ec2__elbv2.deploy-cfn-asg-launch-template-ec2-instances [mismatch]:
  AssertionError: expected propagated tag sim2real:template-check=v1, got None
  (the template declares LaunchTemplateData.TagSpecifications ResourceType=instance with that key; DescribeInstances(InstanceIds=[<asg instance>]) returned Tags without it)
  Expected: Instances[0].Tags contains every key/value from the launch template's instance TagSpecifications (plus the ASG's PropagateAtLaunch tags).
  Actual: the launch-template keys are absent from Tags.

Cause 2 — check autoscaling__ec2__elbv2.e2e-asghc-asg-instance-health-check-failure-lifecycle [error]:
  _PhaseError: [phase 1/6 deploy] LoadBalancerNotFoundException: An error occurred (LoadBalancerNotFound) when calling the DescribeLoadBalancers operation: One or more load balancers not found
  Call: elbv2.describe_load_balancers(LoadBalancerArns=[stack Outputs['NLBArn']]) immediately after the stack reached CREATE_COMPLETE.
  Expected: HTTP 200, one LoadBalancers entry, State.Code 'active'.
  Actual: HTTP 400 LoadBalancerNotFound "One or more load balancers not found".

## Steps to Reproduce
Cause 1:
1. CreateStack with an AWS::EC2::LaunchTemplate whose LaunchTemplateData.TagSpecifications contains {ResourceType: instance, Tags: [{Key: k, Value: v}]} and an AWS::AutoScaling::AutoScalingGroup (Min=Max=Desired=1) referencing that template version.
2. Wait for CREATE_COMPLETE, then poll DescribeAutoScalingGroups until one instance is InService.
3. Call ec2 DescribeInstances(InstanceIds=[that id]) and read Instances[0].Tags — key k is missing; AWS returns k=v.

Cause 2:
1. CreateStack for a VPC + security group + instance profile + launch template + internal AWS::ElasticLoadBalancingV2::LoadBalancer (NLB) + target group + ASG, exporting the NLB ARN as a stack Output.
2. Wait for CREATE_COMPLETE and read Outputs['NLBArn'].
3. Call elbv2 DescribeLoadBalancers(LoadBalancerArns=[that ARN]) — it is refused 400 LoadBalancerNotFound instead of returning the active load balancer.
