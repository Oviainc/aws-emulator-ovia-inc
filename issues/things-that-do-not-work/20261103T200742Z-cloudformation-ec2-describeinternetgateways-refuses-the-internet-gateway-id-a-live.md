Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2.cdk-destroy-removes-every-resource-the-stack-created
Issue-Key: ec2|DescribeInternetGateways|InvalidInternetGatewayID.NotFound
Issue-Commit: 74acd3b5911f8cc01604f18ec0c28de145582648

# DescribeInternetGateways refuses the internet-gateway id a live CloudFormation stack created (InvalidInternetGatewayID.NotFound)

> _Note: this issue was identified against the code base at commit `74acd3b5911f8cc01604f18ec0c28de145582648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInternetGateways

## Description
1 CAUSE.

Cause 1 of 1 - an AWS::EC2::InternetGateway that a stack reports as created cannot be read back by its id.
AWS: DescribeInternetGateways resolves any igw-<id> that exists in the account and region, including one created by a CloudFormation stack, and returns it in InternetGateways; the refusal class InvalidInternetGatewayID.NotFound with the message "The internetGateway ID '<id>' does not exist" is reserved for an id that is absent (fixed text, with the id taken from the request). Smithy: ec2#DescribeInternetGateways takes InternetGatewayIds/Filters and returns InternetGateways; EC2 declares no modeled errors, so the code and message above are recorded behavior only.
Emulator: describing the internet-gateway id the stack reports for its AWS::EC2::InternetGateway resource is refused 400 InvalidInternetGatewayID.NotFound - "The internetGateway ID '<igw id>' does not exist" - while that stack is still live and has not been destroyed. The id is minted and reported but never readable, so the resource is unreachable through its own id.
Why: a client that reads a stack's physical resource ids and then describes them (the standard verify-after-deploy and verify-after-destroy pattern) cannot tell a created gateway from a deleted one: the live gateway answers with the same not-found code a destroyed one would, so post-deploy verification and any dependent call keyed on the gateway id fail.
Fix: the CloudFormation provider for AWS::EC2::InternetGateway and/or the EC2 internet-gateway state it writes into - the gateway the stack creates must be registered in the EC2 store under the id the stack reports, so DescribeInternetGateways resolves it; after a successful destroy the same id must then answer InvalidInternetGatewayID.NotFound.
Note: the already-filed stub issue 20261103T181842Z-ec2-attachinternetgateway-stub.md names ec2 AttachInternetGateway as unimplemented, which may share the underlying gap; this check was not marked blocked by it and the divergence here is on the read path, so it is filed separately and should be re-checked when that stub lands.
Traces: cloudformation__ec2.cdk-destroy-removes-every-resource-the-stack-created.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout; the check is listed under the suite's `### cloudformation__ec2` heading.

cloudformation__ec2.cdk-destroy-removes-every-resource-the-stack-created [error]: the ClientError escaped unhandled from the resource probe - "botocore.exceptions.ClientError: An error occurred (InvalidInternetGatewayID.NotFound) when calling the DescribeInternetGateways operation: The internetGateway ID 'igw-bf37cb4b53ef49c32' does not exist". The check maps AWS::EC2::InternetGateway to the expected absence code InvalidInternetGatewayID.NotFound (its _NOT_FOUND_CODES table) and probes each deployed resource type; the gateway id is one the deployment itself produced, so this id value is a per-run artifact while the behavior - a live, stack-created gateway answering not-found - is the requirement.

Smithy model consulted: services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json (DescribeInternetGateways input InternetGatewayIds/Filters, output InternetGateways, no modeled errors).

## Steps to Reproduce
1. Clone this checkout and replay the behavioral suite (simparity-behavioral-aws-v1) against the booted emulator.
2. Run check cloudformation__ec2.cdk-destroy-removes-every-resource-the-stack-created (group deploy-cdk-l2-vpc), which deploys the cdk-vpc-l2-defaults app and then probes every created resource type by id.
3. Equivalently, without CDK: CreateStack a template containing an AWS::EC2::InternetGateway, wait for CREATE_COMPLETE, read the gateway's physical resource id from DescribeStackResources, and call DescribeInternetGateways(InternetGatewayIds=[<that id>]). It must return the gateway; the emulator answers 400 InvalidInternetGatewayID.NotFound.
4. After DeleteStack completes, the same call must then answer InvalidInternetGatewayID.NotFound.
