Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.deploy-cdk-vpc-public-and-isolated-subnets-route-differently
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.deploy-cdk-vpc-public-and-isolated-subnets-route-differently|run#22|AssertionError|assert igw, f"expected an Internet Gateway attached to {vpc_id}"
Issue-Commit: 74acd3b5911f8cc01604f18ec0c28de145582648

# A stack-created VPC has no internet gateway attachment: DescribeInternetGateways with the attachment.vpc-id filter returns nothing

> _Note: this issue was identified against the code base at commit `74acd3b5911f8cc01604f18ec0c28de145582648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInternetGateways

## Description
1 CAUSE.

Cause 1 of 1 - the attachment between a stack-created internet gateway and its VPC is not observable.
AWS: once an internet gateway is attached to a VPC (by AWS::EC2::VPCGatewayAttachment, or AttachInternetGateway), DescribeInternetGateways filtered on attachment.vpc-id = <vpc id> returns that gateway, with Attachments naming the VPC and State 'available'. Smithy: ec2#DescribeInternetGateways takes Filters (FilterList) and returns InternetGateways; the filter-name domain and the attachment semantics are recorded behavior only (the EC2 model declares no errors and does not enumerate filter names).
Emulator: for a VPC created by a stack that reached CREATE_COMPLETE - whose subnets are readable (DescribeSubnets with vpc-id returns the four subnets) - DescribeInternetGateways(Filters=[{Name: 'attachment.vpc-id', Values: [<vpc id>]}]) returns an empty InternetGateways list. Either the gateway/VPC attachment is never recorded or the attachment.vpc-id filter never matches it.
Why: this is the only supported way to find a VPC's internet gateway when the id is not already known, so every downstream check a client makes - that a public subnet's route table sends 0.0.0.0/0 to the gateway, that an isolated subnet has no default route, that a public/private split exists at all - cannot be made. A stack whose networking is reported as complete is not usable through the EC2 read API.
Fix: EC2 internet-gateway state and its DescribeInternetGateways filtering - record the gateway-to-VPC attachment when the gateway is attached (including on the CloudFormation path) and support the attachment.vpc-id filter over it. Related already-filed stub issue, which may share the underlying gap but does not carry this check: 20261103T181842Z-ec2-attachinternetgateway-stub.md.
Traces: ec2.deploy-cdk-vpc-public-and-isolated-subnets-route-differently.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout; the check is listed under the suite's `### ec2` heading.

ec2.deploy-cdk-vpc-public-and-isolated-subnets-route-differently [mismatch]: "AssertionError: expected an Internet Gateway attached to vpc-00000000000000024", raised at the assertion following `ec2.describe_internet_gateways(Filters=[{"Name": "attachment.vpc-id", "Values": [vpc_id]}])["InternetGateways"]`. Everything before it passed: CreateStack reached CREATE_COMPLETE, the stack outputs yielded VpcId plus two public and two isolated subnet ids, and DescribeSubnets(vpc-id=<vpc>) returned at least four subnets. The vpc id shown is a per-run value; the requirement is that the attached gateway is returned for whatever VPC the stack created.

Smithy model consulted: services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json (DescribeInternetGateways input Filters/InternetGatewayIds, output InternetGateways).

## Steps to Reproduce
1. Clone this checkout and replay the behavioral suite (simparity-behavioral-aws-v1) against the booted emulator.
2. Run check ec2.deploy-cdk-vpc-public-and-isolated-subnets-route-differently.
3. Equivalently: CreateStack a template with an AWS::EC2::VPC, an AWS::EC2::InternetGateway and an AWS::EC2::VPCGatewayAttachment joining them, plus public and isolated subnets; wait for CREATE_COMPLETE and read VpcId from the stack outputs.
4. Call DescribeInternetGateways(Filters=[{Name: 'attachment.vpc-id', Values: [<VpcId>]}]). It must return the attached gateway with an Attachments entry naming that VPC; the emulator returns an empty list.
