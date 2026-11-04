Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__s3__ssm.sep-network-truth-vpc-endpoint-policy-deny-blocks-an-iam-allowed-s3-write-rejected
Issue-Key: ec2__s3__ssm|CreateVpcEndpoint|NotImplemented
Issue-Commit: 04fa8901c0470ed069cb581dfd2246db1b5bced9

# ec2__s3__ssm: CreateVpcEndpoint with route tables and the internet-gateway/route-table delete ops answer NotImplemented, leaving a VPC that cannot be torn down

> _Note: this issue was identified against the code base at commit `04fa8901c0470ed069cb581dfd2246db1b5bced9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcEndpoint

## Description
Cause 1 of 1: the VPC-endpoint create path and the gateway/route-table teardown path are scaffold stubs, so a VPC-endpoint-policy check can neither run nor clean up.
AWS: Smithy (ec2, amazonec2) models CreateVpcEndpoint with RouteTableIds as an ordinary member and declares no modelled errors (200 with a CreateVpcEndpointResult); DetachInternetGateway and DeleteInternetGateway likewise declare no errors and return an empty 200 body once the gateway has no dependencies. After the endpoint, gateway and route table are deleted, DeleteVpc succeeds and DescribeVpcs no longer returns the VPC — DependencyViolation is reserved for a VPC that still has dependencies.
Emulator: CreateVpcEndpoint answers 'NotImplemented' ('operation CreateVpcEndpoint with route table associations is scaffolded but not implemented in this emulator'); DetachInternetGateway, DeleteInternetGateway, DeleteRouteTable and DisassociateRouteTable all answer NotImplemented; DeleteVpc then answers DependencyViolation and the VPC remains describable.
Why: VPC-endpoint policy enforcement over S3 cannot be exercised at all, and every caller that creates a VPC with a gateway is left with resources it has no API to remove — a leak the client cannot repair and a DependencyViolation that misattributes the cause to its own dependencies.
Fix: the CreateVpcEndpoint handler's RouteTableIds branch, and the DetachInternetGateway / DeleteInternetGateway / DeleteRouteTable / DisassociateRouteTable handlers and the VPC dependency bookkeeping they must update, in the ec2 service crate.
Traces: ec2__s3__ssm.sep-network-truth-vpc-endpoint-policy-deny-blocks-an-iam-allowed-s3-write-rejected

## Evidence
Replay detail, verbatim: "ClientError('An error occurred (NotImplemented) when calling the CreateVpcEndpoint operation: operation CreateVpcEndpoint with route table associations is scaffolded but not implemented in this emulator'); teardown could not remove everything the check created: ['igw-detach <igw-id>: NotImplemented', 'igw <igw-id>: NotImplemented', 'vpc <vpc-id>: DependencyViolation', 'vpc <vpc-id> is still describable: [...State: available...]']". Expected: CreateVpcEndpoint 200; DetachInternetGateway/DeleteInternetGateway empty 200; DeleteVpc succeeding and DescribeVpcs no longer returning the VPC. Actual: NotImplemented for the creates and deletes, DependencyViolation on DeleteVpc, VPC still present.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite. 2. Replay ec2__s3__ssm.sep-network-truth-vpc-endpoint-policy-deny-blocks-an-iam-allowed-s3-write-rejected. 3. Directly: CreateVpc, CreateInternetGateway, AttachInternetGateway, CreateRouteTable, then CreateVpcEndpoint with RouteTableIds; then try DetachInternetGateway, DeleteInternetGateway, DeleteRouteTable and DeleteVpc.
