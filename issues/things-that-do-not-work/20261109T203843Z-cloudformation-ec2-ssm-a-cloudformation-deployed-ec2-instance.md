Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__ssm.cfn-ssm-patch-baseline-wrong-tag-filter-negative
Issue-Key: cloudformation__ec2__ssm.cfn-ssm-patch-baseline-wrong-tag-filter-negative|_poll_until#10|Timeout|last = probe()
Issue-Commit: 9354063c6b655c6b4143ce2300841d23d0deff44

# cloudformation__ec2__ssm: a CloudFormation-deployed EC2 instance with an SSM instance profile never reaches PingStatus Online, so AWS::SSM::Association executions never resolve any target

> _Note: this issue was identified against the code base at commit `9354063c6b655c6b4143ce2300841d23d0deff44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceInformation

## Description
1 CAUSE.

Cause 1 of 1 — a CloudFormation stack that creates an EC2 instance with an SSM-capable instance profile plus AWS::SSM::PatchBaseline and AWS::SSM::Association resources never produces a managed node, so the association-execution state the stack is supposed to drive is never reachable.
AWS: once the stack reaches CREATE_COMPLETE and the AWS::EC2::Instance is running with an instance profile whose role carries AmazonSSMManagedInstanceCore, DescribeInstanceInformation filtered on Key='InstanceIds' returns that instance with PingStatus 'Online' (Smithy: com.amazonaws.ssm#PingStatus enum Online | ConnectionLost | Inactive). Each AWS::SSM::Association created by the stack then records an execution: DescribeAssociationExecutions returns an AssociationExecution whose ResourceCountByStatus is a Java-map-style string — '{}' for an association whose tag:'Patch Group' filter matches no instance, and non-'{}' (e.g. '{Success=1}') for the sibling association whose filter matches the instance's actual tag value. All of these are 200 responses; no error code is involved.
Emulator: the check never gets past the registration gate — its 'SSM Agent registers instance as Online' poll (360 s budget) and the association-execution polls (180 s and 120 s budgets) never converge, and the check is cut off by the harness at 180 s before any assertion on ResourceCountByStatus runs. Nothing is asserted about the stack's own status, which the check had already accepted.
Why: a client cannot observe the central behavior of a patch-management estate deployed by CloudFormation — that a tag filter matching no instance resolves to zero targets while a matching one resolves to at least one. Every tag-scoped SSM targeting decision is unobservable, and a negative control (an over-permissive tag filter) cannot be distinguished from a correct one.
Fix: the SSM managed-node registration path and the AWS::SSM::Association execution state — an EC2 instance created by CloudFormation with an SSM-granting instance profile must be projected into SSM's instance-information state as Online, and an association created through CloudFormation must record an execution whose ResourceCountByStatus reflects the instances its Targets tag filter actually matches. The report attributes 32 sibling failures in this same service to the open issues 20261108T120914Z-cloudformation-ec2-ssm-cfn-deployed-ec2-ssm-estates.md and 20261107T075601Z-cloudformation-ec2-ssm-cloudformation-ssm-aws-ssm-maintenancewindowtarget-unrecognised-ec2-ref.md; neither carries this check's replay key.
Traces: cloudformation__ec2__ssm.cfn-ssm-patch-baseline-wrong-tag-filter-negative.

## Evidence
Replay of simparity-behavioral-aws-v1; the check is listed under '### cloudformation__ec2__ssm' in the suite report.

[mismatch] cloudformation__ec2__ssm.cfn-ssm-patch-baseline-wrong-tag-filter-negative: 'Check timed out after 180.0s'.
The check deploys a stack (InstanceRole + instance profile + AWS::EC2::Instance tagged 'Patch Group'=<match>, AWS::SSM::PatchBaseline with PatchGroups=[<match>], AWS::SSM::Association 'AssocGood' targeting tag:'Patch Group'=[<match>] and 'AssocBad' targeting tag:'Patch Group'=[<mismatch>], ImageId from the SSM public parameter /aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64) with a 240 s CFN budget, then polls ssm.describe_instance_information(Filters=[{Key:'InstanceIds',Values:[<id>]}]) for PingStatus == 'Online' (timeout 360 s, interval 10 s), then ssm.describe_association_executions(AssociationId=<AssocGoodId>) for ResourceCountByStatus != '{}' (timeout 180 s) and ssm.describe_association_executions(AssociationId=<AssocBadId>) for any execution (timeout 120 s). The harness cut the check at 180 s, so none of the ResourceCountByStatus assertions ('{}' for the mismatched tag, non-'{}' for the matching tag) or the EC2 DescribeInstances ground-truth cross-check were reached. 32 of the 37 checks under this service heading fail, the other 31 on already-filed causes about CFN-deployed EC2/SSM estates.

Smithy model consulted: services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json (PingStatus enum Online|ConnectionLost|Inactive; DescribeInstanceInformation, DescribeAssociationExecutions).

## Steps to Reproduce
1. Build and boot the emulator from this checkout. 2. Replay simparity-behavioral-aws-v1 and re-run cloudformation__ec2__ssm.cfn-ssm-patch-baseline-wrong-tag-filter-negative. 3. Manually: CreateStack with a template containing an AWS::IAM::Role trusting ec2.amazonaws.com with AmazonSSMManagedInstanceCore, an AWS::IAM::InstanceProfile, an AWS::EC2::Instance tagged 'Patch Group'=<match> using that profile and ImageId from AWS::SSM::Parameter::Value<AWS::EC2::Image::Id> default /aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64, an AWS::SSM::PatchBaseline with PatchGroups=[<match>], and two AWS::SSM::Association resources on AWS-RunPatchBaseline targeting tag:'Patch Group'=[<match>] and tag:'Patch Group'=[<mismatch>]. 4. Wait for CREATE_COMPLETE, then DescribeInstanceInformation(Filters=[{'Key':'InstanceIds','Values':[<instance-id>]}]) and assert PingStatus 'Online'. 5. DescribeAssociationExecutions for each association id and assert ResourceCountByStatus is '{}' for the mismatched-tag association and non-'{}' for the matching-tag one.
