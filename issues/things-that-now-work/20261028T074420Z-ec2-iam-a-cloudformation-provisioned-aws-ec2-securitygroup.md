Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam.ec2ebs-e2e-deploy-ec2-instance-with-ebs-lifecycle
Issue-Key: ec2|DescribeSecurityGroups|InvalidGroup.NotFound
Issue-Commit: 8005689898fcaab18d59205649ae7b06565801fd

# ec2__iam: a CloudFormation-provisioned AWS::EC2::SecurityGroup does not exist in EC2's state — the stack's GetAtt GroupId output answers InvalidGroup.NotFound (1 cause)

> _Note: this issue was identified against the code base at commit `8005689898fcaab18d59205649ae7b06565801fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSecurityGroups

## Description
Cause 1 of 1: a CREATE_COMPLETE CloudFormation stack that declares an AWS::EC2::SecurityGroup publishes an EC2-shaped group id (`sg-` + 17 hex) for `Fn::GetAtt [SecurityGroup, GroupId]`, but no such security group exists in the EC2 service's state, so every EC2 read or write against that id is refused as not-found. In the same stack the AWS::EC2::VPC and AWS::EC2::Subnet resources DO resolve in EC2 (ec2:DescribeSubnets on the published SubnetId answers 200 and its VpcId/AvailabilityZone match the template), so the gap is specific to the security-group resource type, not to the stack as a whole. Failing check: ec2__iam.ec2ebs-e2e-deploy-ec2-instance-with-ebs-lifecycle.

AWS: recorded behavior only for the error code — EC2 speaks the ec2Query protocol and models no errors on these operations (Smithy: CreateSecurityGroup input Description(xmlName GroupDescription, required), GroupName(required), VpcId, TagSpecifications; output GroupId, SecurityGroupArn, Tags; errors: []. DescribeSecurityGroups input GroupIds: GroupIdStringList, GroupNames, Filters, MaxResults @range 5..1000; output SecurityGroups: SecurityGroupList; errors: []). What AWS does: provisioning AWS::EC2::SecurityGroup calls ec2:CreateSecurityGroup and the GroupId that EC2 assigns becomes the resource's PhysicalResourceId and the value of `Ref`/`Fn::GetAtt .GroupId` (Smithy: StackResourceSummary.PhysicalResourceId — "The name or unique identifier that corresponds to a physical instance ID of the resource"). Consequently, while the stack exists, ec2:DescribeSecurityGroups(GroupIds=[<the id the stack published>]) answers HTTP 200 with that group, carrying the template's GroupDescription, its VpcId and its tags, and its egress rules. AWS reserves the not-found refusal for ids that genuinely do not exist: HTTP 400, code `InvalidGroup.NotFound`, message `The security group '<id>' does not exist` — fixed text `The security group '` and `' does not exist`, with the variable part being the group id taken verbatim from the request.

Emulator: the stack reaches CREATE_COMPLETE and cfn:DescribeStacks publishes SecurityGroupId as a correctly shaped `sg-<17 hex>` value, but EC2 has never been told about it. ec2:DescribeSecurityGroups(GroupIds=[<that id>]) is refused with HTTP 400 `InvalidGroup.NotFound`, message `The security group '<id>' does not exist`. The refusal's status, code and wording themselves match AWS; the divergence is that the resource is absent — the id was minted rather than created. Hypothesis: the CloudFormation resource provisioner for AWS::EC2::SecurityGroup synthesizes an id of the right shape (and resolves GetAtt GroupId from it) without routing through EC2's CreateSecurityGroup state, the same class of defect already filed for VPC/Subnet in open issue 20261027T155034Z-cloudformation-ec2-cfn-provisioned-aws-ec2-vpc-and.md — that issue is scoped to AWS::EC2::VPC/AWS::EC2::Subnet, does not mention AWS::EC2::SecurityGroup, and does not carry this check's replay key; in this run VPC and Subnet resolve while the security group does not.

Why: a client cannot use a security group a stack created. Anything that hands the stack's group id to EC2 — RunInstances(SecurityGroupIds=[...]), CreateNetworkInterface, AuthorizeSecurityGroupIngress/Egress, DescribeSecurityGroups — or to a neighbouring service that validates group membership (ELBv2 CreateLoadBalancer, ECS service network configuration, RDS DB instances, Lambda VpcConfig, Auto Scaling launch templates, OpenSearch VpcOptions) is refused with a not-found for a resource CloudFormation reports as created. End-to-end CFN estates therefore fail in their first positive step, before the behavior under test is reached, and the failure is indistinguishable from a genuinely deleted group.

Fix: the CloudFormation provisioner for AWS::EC2::SecurityGroup (and its rule children AWS::EC2::SecurityGroupIngress / AWS::EC2::SecurityGroupEgress, plus the inline SecurityGroupIngress/SecurityGroupEgress properties) must create the group in the EC2 service's own state via the CreateSecurityGroup path and record the GroupId EC2 returns as the resource's PhysicalResourceId and as the value of Ref / GetAtt GroupId / GetAtt GroupName / GetAtt VpcId, instead of synthesizing an `sg-`-shaped id; stack deletion must delete it from that same state. The group's description, VpcId and tags from the template must be stored so DescribeSecurityGroups reports them.

Traces: ec2__iam.ec2ebs-e2e-deploy-ec2-instance-with-ebs-lifecycle (phase 1/6 deploy, DescribeSecurityGroups on the stack's SecurityGroupId output).

## Evidence
Check ec2__iam.ec2ebs-e2e-deploy-ec2-instance-with-ebs-lifecycle [error]
  Stack template (inlined in the check): Resources Vpc(AWS::EC2::VPC 10.0.0.0/16), Igw, IgwAttachment, PublicSubnet(10.0.0.0/24, MapPublicIpOnLaunch true), PublicRouteTable, PublicDefaultRoute, PublicSubnetAssociation, SecurityGroup(AWS::EC2::SecurityGroup, GroupDescription "Security group for EC2 instance", VpcId {Ref: Vpc}, SecurityGroupEgress -1/0.0.0.0/0), PowerUserRole + PowerUserInstanceProfile, EbsVolume(50 GiB gp3). Outputs: VpcId {Ref Vpc}, SubnetId {Ref PublicSubnet}, SecurityGroupId {Fn::GetAtt [SecurityGroup, GroupId]}, EBSVolumeId, IAMRoleName.

  Steps that SUCCEEDED first (so the stack is not wholly unprovisioned): cfn:CreateStack reached CREATE_COMPLETE; cfn:DescribeStacks returned all five Outputs with non-empty values; ec2:DescribeSubnets(SubnetIds=[<SubnetId output>]) answered 200 with VpcId equal to the VpcId output and AvailabilityZone equal to the pinned SubnetAz parameter.

  Expected (real AWS): ec2:DescribeSecurityGroups(GroupIds=[<the SecurityGroupId output>]) answers HTTP 200 with one SecurityGroup whose Description is "Security group for EC2 instance", whose VpcId is the stack's VPC, and which carries the template's Name/sim2real tags and its all-protocol egress rule.
  Actual: botocore.exceptions.ClientError: An error occurred (InvalidGroup.NotFound) when calling the DescribeSecurityGroups operation: The security group 'sg-66d6acac3d18adc0d' does not exist
  (raised as _PhaseError: [phase 1/6 deploy])

  The concrete id 'sg-66d6acac3d18adc0d' is a per-run value; the requirement is that whatever id a CREATE_COMPLETE stack publishes for an AWS::EC2::SecurityGroup resolves in EC2 for as long as the stack exists. The message template above is AWS's: fixed text `The security group '` + `' does not exist`, variable part = the group id from the request.

## Steps to Reproduce
1. cfn:CreateStack with a template declaring an AWS::EC2::VPC (CidrBlock 10.0.0.0/16), an AWS::EC2::Subnet in it, and an AWS::EC2::SecurityGroup with Properties {GroupDescription: "Security group for EC2 instance", VpcId: {Ref: Vpc}, SecurityGroupEgress: [{IpProtocol: "-1", CidrIp: "0.0.0.0/0"}]}, plus an Output SecurityGroupId = {"Fn::GetAtt": ["SecurityGroup", "GroupId"]}. Capabilities=[CAPABILITY_IAM].
2. Poll cfn:DescribeStacks(StackName=<stack>) until StackStatus == CREATE_COMPLETE, and read Stacks[0].Outputs — SecurityGroupId is an `sg-` + 17 hex value.
3. (Control, passes) ec2:DescribeSubnets(SubnetIds=[<SubnetId output>]) — 200, VpcId matches the VpcId output.
4. ec2:DescribeSecurityGroups(GroupIds=[<SecurityGroupId output>]) — refused HTTP 400 InvalidGroup.NotFound "The security group '<id>' does not exist"; real AWS returns the group with 200.
5. Equivalently, ec2:RunInstances(SecurityGroupIds=[<SecurityGroupId output>], SubnetId=<SubnetId output>, ...) or ec2:AuthorizeSecurityGroupIngress(GroupId=<SecurityGroupId output>, ...) is refused with the same not-found for a group CloudFormation reports as created.
6. cfn:ListStackResources(StackName=<stack>) still lists the AWS::EC2::SecurityGroup logical resource with that PhysicalResourceId, confirming the id was minted rather than created in EC2.
