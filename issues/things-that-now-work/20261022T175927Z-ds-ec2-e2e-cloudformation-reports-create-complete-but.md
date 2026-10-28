Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds__ec2.e2e-managed-ad-with-ldaps-and-resets-lifecycle
Issue-Key: ec2|DescribeSubnets|InvalidSubnetID.NotFound
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# ds/ec2 e2e: CloudFormation reports CREATE_COMPLETE but its EC2::Subnet/VPC physical ids are synthesized names EC2 does not know, so DescribeSubnets fails with InvalidSubnetID.NotFound

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSubnets

## Description
Cause 1 of 1: a CloudFormation stack declaring AWS::EC2::VPC and AWS::EC2::Subnet resources reaches CREATE_COMPLETE and publishes Outputs, but the physical resource ids it hands back are synthesized logical names ('<stack-name>-SubnetA-<suffix>') rather than real EC2 resource ids, and no corresponding resource exists in the emulator's EC2 state.
AWS: recorded behavior only for the e2e flow; the ids themselves are modeled - EC2 SubnetId values are 'subnet-' + hex and VpcId values are 'vpc-' + hex, and a stack that reports CREATE_COMPLETE has actually created those resources, so DescribeSubnets(SubnetIds=<stack output>) returns them with their VpcId, AvailabilityZone and CidrBlock. A subnet id that does not exist is refused with HTTP 400 and error code InvalidSubnetID.NotFound - which is exactly what the emulator answered, confirming EC2 has no such resource.
Emulator: cloudformation.create_stack of a three-subnet VPC template settles at CREATE_COMPLETE and DescribeStacks Outputs carry SubnetIds='sim2real-ds-ldaps-38c2d66eae40-SubnetA-FB3CF63CCB62,...'; ec2.describe_subnets(SubnetIds=[those]) then fails with InvalidSubnetID.NotFound: "The subnet ID 'sim2real-ds-ldaps-38c2d66eae40-SubnetA-FB3CF63CCB62' does not exist".
Why: every client that provisions a network with CloudFormation and feeds the stack Outputs into EC2 or a network-consuming service (here ds.CreateMicrosoftAD's VpcSettings) breaks at the first use of the output: the stack claims success while the account holds no VPC or subnet, so the whole deploy -> solve -> destroy shape is unreachable and a successful stack no longer implies existing resources.
Fix: CloudFormation's resource provisioning for AWS::EC2::VPC and AWS::EC2::Subnet - create the resources through the EC2 state and record the real allocated ids as the physical resource ids (and hence in Ref/Outputs), instead of synthesizing '<stack>-<logical-id>-<suffix>' names.
Related open issues (NOT the same work item, different service tokens, named here for the fixer): 20261022T154846Z-ec2-emr-iam-cloudformation-network-stack-outputs-carry-synthesized-logical-names.md and 20261022T154846Z-ec2-emr-cloudformation-stack-outputs-return-synthesized-logical-names-instead.md describe the same class of defect reached from other suites; neither carries this check's replay key.
Traces: ds__ec2.e2e-managed-ad-with-ldaps-and-resets-lifecycle (phase 1/4 deploy).

Service attribution note: the failing check id is 'ds__ec2.e2e-managed-ad-with-ldaps-and-resets-lifecycle', whose leading token is 'ds__ec2' (a ds+ec2 joint check); the defective component is CloudFormation's EC2 resource provisioning, and the observable failure is on ec2.DescribeSubnets.

## Evidence
Behavioral replay, check ds__ec2.e2e-managed-ad-with-ldaps-and-resets-lifecycle:
_PhaseError: [phase 1/4 deploy] ClientError: An error occurred (InvalidSubnetID.NotFound) when calling the DescribeSubnets operation: The subnet ID 'sim2real-ds-ldaps-38c2d66eae40-SubnetA-FB3CF63CCB62' does not exist
The phase had already polled the stack to CREATE_COMPLETE and read its Outputs (VpcId, SubnetIds) via DescribeStacks, so the ids above ARE what CloudFormation published. Expected: DescribeSubnets returns three subnets, all with VpcId equal to the stack's VpcId output, spanning three AvailabilityZones. Observed: a 400 InvalidSubnetID.NotFound for the first output id, i.e. the stack's physical ids are names, not 'subnet-...' ids, and EC2 holds no matching resource.

## Steps to Reproduce
1. Boot the emulator; point botocore `cloudformation` and `ec2` clients at it.
2. create_stack with a template declaring one AWS::EC2::VPC (10.168.0.0/16) and three AWS::EC2::Subnet resources (one per AZ via Fn::GetAZs), with Outputs VpcId = Ref Vpc and SubnetIds = Fn::Join of the three subnet Refs.
3. Poll describe_stacks until StackStatus == CREATE_COMPLETE and read the Outputs.
4. ec2.describe_subnets(SubnetIds=<the SubnetIds output, split on ','>) -> expect the three subnets; observe 400 InvalidSubnetID.NotFound naming '<stack-name>-SubnetA-<suffix>'.
5. Also note ec2.describe_vpcs(VpcIds=[<VpcId output>]) and confirm the VPC output id is likewise not a real 'vpc-...' resource.
