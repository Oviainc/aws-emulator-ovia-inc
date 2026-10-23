Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.list-syslog-configurations-reflects-put-and-delete
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|CreateVpcEndpoint|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# EC2 CreateVpcEndpoint is an unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcEndpoint

## Description
Cause 1 of 1: CreateVpcEndpoint answers NotImplemented.
AWS: CreateVpcEndpoint creates an interface/gateway VPC endpoint for the named service and returns the VpcEndpoint (VpcEndpointId vpce-..., State pending/available, DnsEntries); it is the prerequisite for any private-endpoint flow. Smithy (ec2): CreateVpcEndpoint -> CreateVpcEndpointResult over the awsQuery protocol; EC2 reports client faults as Client.<Code> - NotImplemented is not a modeled EC2 error.
Emulator: ClientError code 'NotImplemented', message 'operation CreateVpcEndpoint is scaffolded but not implemented in this emulator'.
Why: nothing that reaches a service over a VPC endpoint can be set up - in this suite it blocks the CloudWatch Logs syslog-configuration lifecycle, which needs an endpoint to attach a listener to. Clients that provision private connectivity fail at the first call.
Fix: implement the CreateVpcEndpoint handler in the ec2 service crate's ops module (mint vpce- ids, persist the endpoint and its service/subnet associations so DescribeVpcEndpoints and the delete path read it back). Sibling EC2 stubs already have their own issues (CreateFlowLogs, DeleteFlowLogs, DescribeFlowLogs, CreateInternetGateway, AuthorizeSecurityGroupIngress, DescribeNetworkInterfaces, DescribeVpcEndpointServices); CreateVpcEndpoint is not among them.
Traces: logs.list-syslog-configurations-reflects-put-and-delete.

## Evidence
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateVpcEndpoint operation: operation CreateVpcEndpoint is scaffolded but not implemented in this emulator. Expected: 200 with a VpcEndpoint (vpce-... id, State pending/available).

## Steps to Reproduce
ec2.create_vpc, then ec2.create_vpc_endpoint(VpcId=<vpc>, ServiceName='com.amazonaws.us-west-2.logs', VpcEndpointType='Interface', SubnetIds=[...]) -> NotImplemented.
