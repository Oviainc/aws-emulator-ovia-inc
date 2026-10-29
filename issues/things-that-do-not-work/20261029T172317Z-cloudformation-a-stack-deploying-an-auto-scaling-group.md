Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected
Issue-Key: autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal|_deploy_cfn#9|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected|_in_service_ids#18|AssertionError|assert groups, f"auto scaling group {asg_name!r} not found"
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# CloudFormation: a stack deploying an Auto Scaling group returns Outputs with no OutputValue, and reports the stack created while the AWS::AutoScaling::AutoScalingGroup does not exist in Auto Scaling (2 causes)

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
TWO independent CloudFormation root causes, both reached by deploying an ASG template and then reading the result back. BOTH must be closed before submitting. Each one breaks the hand-off from a deployed stack to the service it provisioned, so the ASG update-policy scenarios (rolling update and replacing update) stop at step one.

Cause 1 of 2: DescribeStacks returns Stacks[].Outputs[] entries with no OutputValue member.
AWS: each declared Output comes back as {OutputKey, OutputValue[, Description, ExportName]} with OutputValue fully resolved (Ref/Fn::GetAtt/Fn::Sub evaluated) once the stack is CREATE_COMPLETE; OutputValue is the only way the resolved value reaches the caller. Recorded behavior only for the exact output set.
Emulator: the Outputs entries exist but have no OutputValue key, so the standard `{o['OutputKey']: o['OutputValue'] for o in stack.get('Outputs', [])}` idiom raises KeyError: 'OutputValue' and the deployment is unusable past the deploy step.
Why: Outputs are the only contract a template exposes to its caller; a client that just deployed a stack cannot read the ASG name, the launch-template id or anything else out of it, so every post-deploy call is made with nothing.
Fix: Outputs resolution/serialization in the CloudFormation stack handler (evaluate the intrinsic and emit OutputValue). Related open issues name this family but do NOT carry this check's replay key: 20261024T015148Z-cloudformation-stack-outputs-are-unusable-and-create-complete.md (its Cause 1) and 20261023T205437Z-cloudformation-reports-create-complete-for-resources-it-never.md (its Cause 1).
Traces: autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal.

Cause 2 of 2: a stack whose template declares an AWS::AutoScaling::AutoScalingGroup reports the resource created, but the group does not exist in Auto Scaling.
AWS: once the stack reports the resource CREATE_COMPLETE, the resource's physical id IS the Auto Scaling group name, and autoscaling DescribeAutoScalingGroups(AutoScalingGroupNames=[<that name>]) returns exactly one group (Smithy, Auto Scaling 2011-01-01: DescribeAutoScalingGroups -> AutoScalingGroupsType.AutoScalingGroups).
Emulator: DescribeAutoScalingGroups for the name the stack reports returns an EMPTY AutoScalingGroups list — the check fails with `AssertionError: auto scaling group '<stack-name>-AutoScalingGroup-<hex>' not found`. The group name in that message is a per-run physical id; the requirement is that whatever physical id the stack reports for the logical ASG resource is describable in Auto Scaling.
Why: the terminal stack status lies. Every client that gates on a completed stack and then drives the group it declares (update policies, desired-capacity changes, signal-based rolling updates) operates on a group that does not exist, and the failure surfaces in Auto Scaling, far from its cause.
Fix: the CloudFormation resource provider for AWS::AutoScaling::AutoScalingGroup — it must create the group through the autoscaling state it later reports, and propagate the group name as the resource's physical id. Related open issue 20261022T175547Z-autoscaling-auto-scaling-4-fidelity-divergences-cloudformation-asg-never.md names this family (CloudFormation ASG never provisioned into Auto Scaling) but does not carry this check's replay key.
Traces: autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected.

## Evidence
Cause 1 — expected Outputs entries shaped {OutputKey, OutputValue}; actual: KeyError: 'OutputValue' raised at `outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}` inside the check's _deploy_cfn helper, immediately after the stack deploy.
Cause 2 — expected autoscaling DescribeAutoScalingGroups(AutoScalingGroupNames=[<stack's ASG physical id>]) to return one group; actual an empty AutoScalingGroups list: AssertionError: auto scaling group 'sim2real-xsvc-rpa-fc1319d17f41-AutoScalingGroup-AA8FD65C37CF' not found (the name is this run's physical id).

## Steps to Reproduce
Replay the two check ids in trace_ids. Minimal repros:
1. Cause 1: cloudformation CreateStack with any template that has an Outputs section, wait for CREATE_COMPLETE, then DescribeStacks and read Stacks[0].Outputs[0]['OutputValue'].
2. Cause 2: CreateStack with a template declaring AWS::EC2::LaunchTemplate + AWS::AutoScaling::AutoScalingGroup (DesiredCapacity=2, VPCZoneIdentifier from the stack's subnets), wait until the stack/resource reports complete, read the ASG resource's physical id via DescribeStackResources or an Output, then call autoscaling DescribeAutoScalingGroups(AutoScalingGroupNames=[<that id>]).
