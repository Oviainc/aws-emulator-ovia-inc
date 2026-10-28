Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudformation__cloudwatch__ec2.deploy-autoscaling-3-cdk-l2-asg-target-tracking-deploys-runs-and-drifts
Issue-Key: autoscaling__cloudformation__cloudwatch__ec2.deploy-autoscaling-3-cdk-l2-asg-target-tracking-deploys-runs-and-drifts|_wait_stack#32|Timeout|time.sleep(10.0)
Issue-Commit: 68e4a9e742a6835063083b9abc795c05ac8a482e

# autoscaling__cloudformation__cloudwatch__ec2: the CDK L2 ASG target-tracking deploy/run/drift scenario never produces a terminal result

> _Note: this issue was identified against the code base at commit `68e4a9e742a6835063083b9abc795c05ac8a482e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1 — the end-to-end CDK-synthesised Auto Scaling group scenario (CreateStack of the synthesised template, then ASG capacity, then CloudWatch target-tracking alarms, then DetectStackDrift/DescribeStackResourceDrifts) hangs: nothing the check waits on ever becomes observable, so the check is cut off at the suite's 60 s ceiling with no terminal state at all. Failing check: autoscaling__cloudformation__cloudwatch__ec2.deploy-autoscaling-3-cdk-l2-asg-target-tracking-deploys-runs-and-drifts.

AWS: every wait in this check polls an observable condition that real AWS reaches. CreateStack of the CDK-synthesised template (a public-subnet-only VPC with no NAT, a security group, an AWS::EC2::LaunchTemplate, an AWS::AutoScaling::AutoScalingGroup with MinSize 1 / MaxSize 2 and a rolling UpdatePolicy, plus the target-tracking policy from scaleOnCpuUtilization(50)) settles at StackStatus CREATE_COMPLETE and DescribeStacks exposes Outputs AsgName, VpcId and LaunchTemplateId with real identifiers; DescribeAutoScalingGroups then reports between MinSize and MaxSize InService instances that DescribeInstances reports 'running' and whose LaunchTemplate is the stack's; Auto Scaling itself creates the target-tracking CloudWatch alarms; DetectStackDrift on the freshly created stack reaches DetectionStatus DETECTION_COMPLETE with StackDriftStatus IN_SYNC, and after an out-of-band UpdateAutoScalingGroup(MaxSize=3) it reaches DETECTION_COMPLETE with StackDriftStatus DRIFTED and DescribeStackResourceDrifts reports exactly the ASG logical id MODIFIED with one PropertyDifference at PropertyPath '/MaxSize' (ExpectedValue '2', ActualValue '3', DifferenceType NOT_EQUAL); DeleteStack then settles at DELETE_COMPLETE. Smithy: com.amazonaws.cloudformation#StackStatus = [CREATE_IN_PROGRESS, CREATE_FAILED, CREATE_COMPLETE, ROLLBACK_IN_PROGRESS, ROLLBACK_FAILED, ROLLBACK_COMPLETE, DELETE_IN_PROGRESS, DELETE_FAILED, DELETE_COMPLETE, ...]; com.amazonaws.cloudformation#StackDriftStatus = [DRIFTED, IN_SYNC, UNKNOWN, NOT_CHECKED]. The per-resource drift property semantics are recorded behavior only.

Emulator: the check produces no result at all — the replay report's only detail is "Error: Check timed out after 60.0s". The check's own budgets are far larger (240 s synth, 720 s create, 300 s instances-running, 300 s drift, 720 s delete), so nothing it polls had become observable when the suite's ceiling cut it off: no terminal StackStatus, no Outputs, no InService instance, no DETECTION_COMPLETE. Hypothesis: the stack create never leaves CREATE_IN_PROGRESS (or drift detection never leaves DETECTION_IN_PROGRESS), the same shape the emulator already shows for other CDK-driven scenarios.

Why: a client deploying an ASG the mainstream way — cdk synth, then CloudFormation — gets a stack that never settles, so it cannot read AsgName/VpcId/LaunchTemplateId, cannot see the group run capacity, cannot see the target-tracking alarms Auto Scaling is supposed to create, cannot detect drift after an out-of-band change, and cannot delete the stack. A deploy loop that simply returns against AWS blocks forever against the emulator, which is strictly worse than a refusal: there is no error to branch on.

Fix: the CloudFormation stack-engine's create/converge loop and its drift-detection path (whatever leaves the stack and the drift detection in a non-terminal state), together with the AWS::AutoScaling::AutoScalingGroup / AWS::EC2::LaunchTemplate provisioners and the Auto Scaling target-tracking policy's CloudWatch alarm creation that the later phases read. Related open issues of the same shape that do NOT carry this check: 20261026T034404Z-cloudformation-a-cdk-deployed-function-table-lambdarestapi-stack.md and 20261028T094216Z-cloudformation-ec2-the-cdk-vpc-diff-drift-scenario.md.

Traces: autoscaling__cloudformation__cloudwatch__ec2.deploy-autoscaling-3-cdk-l2-asg-target-tracking-deploys-runs-and-drifts.

## Evidence
Expected (AWS): the scenario completes — CreateStack settles at 'CREATE_COMPLETE' with Outputs AsgName/VpcId/LaunchTemplateId populated; DescribeAutoScalingGroups reports 1..2 InService instances that DescribeInstances reports 'running'; DetectStackDrift reaches DetectionStatus 'DETECTION_COMPLETE' with StackDriftStatus 'IN_SYNC', then 'DRIFTED' after UpdateAutoScalingGroup(MaxSize=3), with DescribeStackResourceDrifts reporting the ASG MODIFIED and exactly one PropertyDifference at '/MaxSize' (ExpectedValue '2', ActualValue '3', DifferenceType 'NOT_EQUAL'); DeleteStack settles at 'DELETE_COMPLETE'.

Actual (emulator): no terminal state is ever observed. Replay detail, verbatim and complete:
  Error: Check timed out after 60.0s
The check's internal ceilings (_CREATE_TIMEOUT_S=720, _INSTANCE_RUNNING_TIMEOUT_S=300, _DRIFT_TIMEOUT_S=300, _DELETE_TIMEOUT_S=720) were never reached, so the hang is in a poll that never satisfied its condition, not in a budget expiry the check would have reported with its own message.

## Steps to Reproduce
1. Boot the emulator and point the CDK/botocore clients at it.
2. cdk synth the inline app: a VPC (nat_gateways=0, PUBLIC subnets only, two AZs derived from DescribeInstanceTypeOfferings for t3.micro), a SecurityGroup, an ec2.LaunchTemplate (t3.micro, latest AL2023), an autoscaling.AutoScalingGroup (min 1, max 2, rolling UpdatePolicy) and asg.scale_on_cpu_utilization('Cpu', target_utilization_percent=50), with Outputs AsgName/VpcId/LaunchTemplateId.
3. cfn.create_stack(StackName='cdkasg1', TemplateBody=<synthesised template with the BootstrapVersion parameter/rule stripped>, Capabilities=['CAPABILITY_IAM']).
4. Poll cfn.describe_stacks(...)['Stacks'][0]['StackStatus'] — it never reaches a terminal value, so steps 5-7 are unreachable.
5. cfn.detect_stack_drift(StackName=...) then poll describe_stack_drift_detection_status until DetectionStatus != 'DETECTION_IN_PROGRESS'.
6. autoscaling.update_auto_scaling_group(MaxSize=3), re-detect drift, cfn.describe_stack_resource_drifts(StackResourceDriftStatusFilters=['MODIFIED']).
7. cfn.delete_stack(...) and poll to 'DELETE_COMPLETE'.
8. Equivalently: replay check autoscaling__cloudformation__cloudwatch__ec2.deploy-autoscaling-3-cdk-l2-asg-target-tracking-deploys-runs-and-drifts.
