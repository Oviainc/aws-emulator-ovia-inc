Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2.cdk-destroy-removes-every-resource-the-stack-created cloudformation__ec2.cdk-diff-sees-a-template-change-and-is-blind-to-resource-drift
Issue-Key: ec2|DescribeVpcs|InvalidVpcID.NotFound
Issue-Key: ec2|ModifySubnetAttribute|InvalidSubnetID.NotFound
Issue-Commit: 72e40d23ae86c919f91b622f812baa35119cc1ff

# cloudformation__ec2: CFN-provisioned AWS::EC2::VPC and AWS::EC2::Subnet resources are never created in EC2's own state — their EC2-shaped PhysicalResourceIds answer InvalidVpcID.NotFound / InvalidSubnetID.NotFound (1 cause)

> _Note: this issue was identified against the code base at commit `72e40d23ae86c919f91b622f812baa35119cc1ff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a stack that declares AWS::EC2::VPC and AWS::EC2::Subnet reaches CREATE_COMPLETE and reports EC2-shaped PhysicalResourceIds (`vpc-` / `subnet-` + 17 hex) for them, but the corresponding resources do not exist in the EC2 service's state, so every EC2 read or write against those ids is refused as not-found.

AWS: recorded behavior only for the error codes — EC2 uses the ec2Query protocol and models no errors on these operations (Smithy: DescribeVpcs input VpcIds: VpcIdStringList, output Vpcs: VpcList, errors: []; ModifySubnetAttribute input SubnetId required, output Unit, errors: []). What AWS does: provisioning AWS::EC2::VPC calls ec2:CreateVpc and AWS::EC2::Subnet calls ec2:CreateSubnet, and the id EC2 assigns becomes the resource's PhysicalResourceId (Smithy: StackResourceSummary.PhysicalResourceId — "The name or unique identifier that corresponds to a physical instance ID of the resource") and the value of Ref. Consequently, for a CREATE_COMPLETE stack, ec2:DescribeVpcs(VpcIds=[<PhysicalResourceId of the VPC>]) returns that VPC with 200, and ec2:ModifySubnetAttribute(SubnetId=<a subnet id the stack published>, MapPublicIpOnLaunch={Value:false}) succeeds with 200 and is observable on ec2:DescribeSubnets. The not-found refusals AWS reserves for ids that genuinely do not exist are HTTP 400 with code InvalidVpcID.NotFound, message `The vpc ID '<id>' does not exist`, and code InvalidSubnetID.NotFound, message `The subnet ID '<id>' does not exist` (fixed text `The vpc ID '`/`The subnet ID '` and `' does not exist`; the variable part is the id taken from the request).

Emulator: the stack is reported CREATE_COMPLETE and ListStackResources returns exactly the declared resource-type counts with plausible `vpc-<17 hex>` / `subnet-<17 hex>` PhysicalResourceIds, and the stack's Outputs publish those same subnet ids — but EC2 does not know them. ec2:DescribeVpcs(VpcIds=[<the stack's VPC PhysicalResourceId>]) is refused with InvalidVpcID.NotFound `The vpc ID 'vpc-…' does not exist`, and ec2:ModifySubnetAttribute(SubnetId=<a subnet id the stack published>) is refused with InvalidSubnetID.NotFound `The subnet ID 'subnet-…' does not exist`. The refusal wording itself matches AWS; the divergence is that the resource is absent, i.e. the id was minted rather than created. Hypothesis: the CloudFormation resource provisioner for these EC2 types generates an id of the right shape without going through EC2's CreateVpc/CreateSubnet state, so nothing is registered in the EC2 store.

Why: a client cannot do anything with a VPC or subnet a stack created. Positive controls fail before the behavior under test is ever reached: a teardown check cannot tell "destroyed" from "never created" because the resource is already not-found while the stack is CREATE_COMPLETE, and a drift check cannot make the out-of-band resource mutation whose drift it is meant to observe. More broadly, every CDK/CFN network estate is unusable — anything that hands a Ref'd VPC or subnet id to EC2 (ModifySubnetAttribute, DescribeSubnets, RunInstances, security-group rules) or to another service (ELBv2, ECS, RDS DB subnet groups, Lambda VpcConfig, ASG) is refused, and CloudFormation's own drift detection has no live resource to compare the template against.

Fix: the CloudFormation provisioner for AWS::EC2::VPC and AWS::EC2::Subnet (and the VPC child types it creates alongside them — RouteTable, InternetGateway, EIP, NatGateway, Route, SubnetRouteTableAssociation, VPCGatewayAttachment) must create the resource in the EC2 service's state and record the id EC2 returns as PhysicalResourceId / Ref, rather than synthesizing an EC2-shaped id; stack deletion must then delete it from that same state. Related OPEN issue 20261025T150742Z-cloudformation-ec2-rds-aws-ec2-subnet-s-physical.md describes the adjacent case where the subnet's PhysicalResourceId is a stack-generated NAME; here the id has the correct EC2 form but no resource backs it, and that issue does not carry these checks' replay keys.

Traces: cloudformation__ec2.cdk-destroy-removes-every-resource-the-stack-created (DescribeVpcs), cloudformation__ec2.cdk-diff-sees-a-template-change-and-is-blind-to-resource-drift (ModifySubnetAttribute).

## Evidence
Check cloudformation__ec2.cdk-destroy-removes-every-resource-the-stack-created [error]
  Expected (real AWS): immediately after `cdk deploy` of an L2 VPC app reaches CREATE_COMPLETE, every directly probeable resource in cfn:ListStackResources is readable — ec2:DescribeVpcs(VpcIds=[<VPC PhysicalResourceId>]) answers 200 with that VPC (the positive control that makes the later post-destroy InvalidVpcID.NotFound meaningful).
  Actual: botocore.exceptions.ClientError: An error occurred (InvalidVpcID.NotFound) when calling the DescribeVpcs operation: The vpc ID 'vpc-339fa1cdf9b24ddfd' does not exist

Check cloudformation__ec2.cdk-diff-sees-a-template-change-and-is-blind-to-resource-drift [error]
  Expected (real AWS): a public subnet id published in the deployed stack's Outputs accepts ec2:ModifySubnetAttribute(SubnetId=<id>, MapPublicIpOnLaunch={Value: False}) with 200, and the flip is then visible on ec2:DescribeSubnets (so that cfn:DetectStackDrift can report the subnet MODIFIED at /MapPublicIpOnLaunch).
  Actual: botocore.exceptions.ClientError: An error occurred (InvalidSubnetID.NotFound) when calling the ModifySubnetAttribute operation: The subnet ID 'subnet-98c813b033c5564e8' does not exist

  Root fact common to both: the stack is CREATE_COMPLETE and publishes EC2-shaped ids (`vpc-`/`subnet-` + 17 hex) as PhysicalResourceId and in its Outputs, yet EC2 has no such resource. The specific ids above are per-run values; the requirement is that an id a CFN stack publishes for an AWS::EC2::VPC / AWS::EC2::Subnet resolves in EC2 while the stack exists.

## Steps to Reproduce
1. Deploy a stack that declares an AWS::EC2::VPC with two AZs of subnets (e.g. `cdk deploy` of a CDK L2 `ec2.Vpc(max_azs=2)` app, or cfn:CreateStack with Vpc{Type:AWS::EC2::VPC, Properties:{CidrBlock:10.0.0.0/16}} plus Subnet{Type:AWS::EC2::Subnet, Properties:{VpcId:{Ref:Vpc}, CidrBlock:10.0.0.0/24, AvailabilityZone:<az>, MapPublicIpOnLaunch:true}}).
2. Wait for CREATE_COMPLETE, then cfn:ListStackResources(StackName=<stack>) and note the PhysicalResourceId of the AWS::EC2::VPC and AWS::EC2::Subnet resources (both are `vpc-`/`subnet-` + 17 hex).
3. ec2:DescribeVpcs(VpcIds=[<VPC PhysicalResourceId>]) — refused 400 InvalidVpcID.NotFound "The vpc ID '<id>' does not exist"; AWS returns the VPC.
4. ec2:ModifySubnetAttribute(SubnetId=<subnet PhysicalResourceId or the id published in Stacks[0].Outputs>, MapPublicIpOnLaunch={"Value": false}) — refused 400 InvalidSubnetID.NotFound "The subnet ID '<id>' does not exist"; AWS answers 200 and ec2:DescribeSubnets then reports MapPublicIpOnLaunch=false.
5. cfn:DetectStackDrift(StackName=<stack>) cannot report the subnet MODIFIED at /MapPublicIpOnLaunch because the mutation in step 4 could never be applied.
