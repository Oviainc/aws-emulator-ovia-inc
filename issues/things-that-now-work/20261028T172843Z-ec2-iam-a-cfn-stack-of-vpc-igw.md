Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam.ec2ebs-e2e-deploy-ec2-instance-with-ebs-lifecycle
Issue-Key: ec2__iam.ec2ebs-e2e-deploy-ec2-instance-with-ebs-lifecycle|phase#4|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 68e4a9e742a6835063083b9abc795c05ac8a482e

# ec2__iam: a CFN stack of VPC/IGW/Subnet/RouteTable/SecurityGroup + IAM role/profile + EBS volume ends ROLLBACK_FAILED instead of CREATE_COMPLETE

> _Note: this issue was identified against the code base at commit `68e4a9e742a6835063083b9abc795c05ac8a482e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1 — CreateStack of an EC2-plus-IAM "compute-and-data" estate never reaches CREATE_COMPLETE, and the failed create's rollback also fails, parking the stack in ROLLBACK_FAILED. Failing check: ec2__iam.ec2ebs-e2e-deploy-ec2-instance-with-ebs-lifecycle.

AWS: CloudFormation CreateStack on this template (Smithy: com.amazonaws.cloudformation#StackStatus enum = CREATE_IN_PROGRESS, CREATE_FAILED, CREATE_COMPLETE, ROLLBACK_IN_PROGRESS, ROLLBACK_FAILED, ROLLBACK_COMPLETE, ...) provisions AWS::EC2::VPC, AWS::EC2::InternetGateway, AWS::EC2::VPCGatewayAttachment, AWS::EC2::Subnet, AWS::EC2::RouteTable, AWS::EC2::Route, AWS::EC2::SubnetRouteTableAssociation, AWS::EC2::SecurityGroup, AWS::IAM::Role, AWS::IAM::InstanceProfile and AWS::EC2::Volume, and the stack settles at CREATE_COMPLETE with Outputs VpcId, SubnetId, SecurityGroupId, EBSVolumeId and IAMRoleName carrying real EC2/IAM identifiers. Two AWS invariants matter here: (a) every resource type in this template is a supported, long-standing type, so the create succeeds; (b) when a create DOES fail, the default OnFailure=ROLLBACK path deletes what was created and the stack settles at ROLLBACK_COMPLETE. ROLLBACK_FAILED is reserved for the case where the rollback's own deletions fail — AWS documents it as requiring an explicit continue/retry, and it is never the outcome of a clean create failure. (Resource-level success criteria are recorded behavior only: CloudFormation's resource provisioning is not in the Smithy model.)

Emulator: the check polls for any terminal state and the emulator's stack settles at ROLLBACK_FAILED — "stack sim2real-ec2ebs-<id> reached 'ROLLBACK_FAILED', not CREATE_COMPLETE". So the create of this template fails (no Outputs, no VPC/subnet/security-group/volume to go on with) AND the rollback that follows also fails, which is the worst of the two wrong answers: the stack is neither usable nor cleaned up. The stack name and id in the message are per-run values; the requirement is the status, not the name.

Why: a client deploying an EC2 estate through CloudFormation — the normal way an instance, its network and its EBS volume are provisioned — gets no resources and no usable stack. Every downstream phase of the scenario (launch an instance into the subnet, attach the 50 GiB gp3 volume at /dev/sdf, poll the attachment to 'attached', filter describe_volumes by attachment.instance-id, read the instance profile's roles) is unreachable because the estate was never created. Worse, ROLLBACK_FAILED means the client cannot even retry the deploy: in AWS semantics that stack must be force-deleted or continued by hand, so a CI pipeline that would simply re-run against AWS is wedged against the emulator.

Hypothesis: the create failure is upstream of CloudFormation — this suite's own report marks EC2 CreateInternetGateway and CreateRouteTable as unimplemented scaffold stubs (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_internet_gateway.rs, .../create_route_table.rs), and this template declares both an InternetGateway and a RouteTable. Those stubs have their own issues; what is NOT covered by them is the stack's terminal state, which must be ROLLBACK_COMPLETE (or CREATE_COMPLETE once the resources exist), never ROLLBACK_FAILED.

Fix: the CloudFormation stack-engine's rollback path (the component that walks a failed create's provisioned resources back out and sets the stack's terminal StackStatus) plus the EC2 resource provisioners for AWS::EC2::InternetGateway / AWS::EC2::RouteTable that the create needs. Related open issues that do NOT cover this check: 20261025T015306Z-cloudformation-never-rolls-a-failed-stack-back-failed.md, 20261027T155034Z-cloudformation-ec2-cfn-provisioned-aws-ec2-vpc-and.md and 20261028T074420Z-ec2-iam-a-cloudformation-provisioned-aws-ec2-securitygroup.md describe neighbouring CFN/EC2 provisioning gaps but none of them carries this check's replay key or this terminal-state divergence.

Traces: ec2__iam.ec2ebs-e2e-deploy-ec2-instance-with-ebs-lifecycle (phase 1/6 deploy).

## Evidence
Expected (AWS): cloudformation.CreateStack of deployments/ec2-ebs-instance-estate/template.json settles at StackStatus 'CREATE_COMPLETE', and DescribeStacks returns Outputs VpcId / SubnetId / SecurityGroupId / EBSVolumeId / IAMRoleName with real EC2 and IAM identifiers. If the create had to fail, the stack settles at 'ROLLBACK_COMPLETE'.

Actual (emulator): the stack settles at StackStatus 'ROLLBACK_FAILED'. Replay detail:
  AssertionError: [phase 1/6 deploy] stack sim2real-ec2ebs-76e5459e0b9c reached 'ROLLBACK_FAILED', not CREATE_COMPLETE -- read its stack events for the resource that failed
(the fixed text of that assertion is the check's; 'sim2real-ec2ebs-76e5459e0b9c' is a per-run stack name and is not part of the requirement. The requirement is the status value: CREATE_COMPLETE, or at worst ROLLBACK_COMPLETE — never ROLLBACK_FAILED.)

Smithy: com.amazonaws.cloudformation#StackStatus admits both ROLLBACK_COMPLETE and ROLLBACK_FAILED; the emulator picks the latter for an ordinary failed create.

## Steps to Reproduce
1. Boot the emulator and point a botocore client at it.
2. cfn.create_stack(StackName='s1', TemplateBody=<deployments/ec2-ebs-instance-estate/template.json>, Parameters=[{NameTag}, {SubnetAz: an AZ offering t3.micro}], Capabilities=['CAPABILITY_IAM']). The template declares AWS::EC2::VPC, InternetGateway, VPCGatewayAttachment, Subnet, RouteTable, Route, SubnetRouteTableAssociation, SecurityGroup, AWS::IAM::Role, AWS::IAM::InstanceProfile and a 50 GiB gp3 AWS::EC2::Volume.
3. Poll cfn.describe_stacks(StackName='s1')['Stacks'][0]['StackStatus'] until terminal.
4. Observe 'ROLLBACK_FAILED'; AWS reaches 'CREATE_COMPLETE' (and 'ROLLBACK_COMPLETE' if the create is made to fail).
5. Equivalently: replay check ec2__iam.ec2ebs-e2e-deploy-ec2-instance-with-ebs-lifecycle.
