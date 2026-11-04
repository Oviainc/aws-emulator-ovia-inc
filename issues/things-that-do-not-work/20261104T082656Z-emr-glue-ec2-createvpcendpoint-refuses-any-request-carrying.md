Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue.emr-spark-step-on-an-unregistered-glue-table-fails emr__glue.on-cluster-add-partition-lands-in-the-glue-catalog
Issue-Key: emr__glue|CreateVpcEndpoint|NotImplemented
Issue-Commit: ed67363a200a02a71118cd5e7b7585c85db4efeb

# emr__glue: EC2 CreateVpcEndpoint refuses any request carrying RouteTableIds with 501 NotImplemented, so no gateway endpoint can be created and the EMR+Glue checks never reach their cluster

> _Note: this issue was identified against the code base at commit `ed67363a200a02a71118cd5e7b7585c85db4efeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcEndpoint

## Description
1 cause.

Cause 1 of 1: CreateVpcEndpoint with route-table associations is an unimplemented scaffold branch.
AWS: Smithy: CreateVpcEndpoint input VpcId (required), ServiceName, VpcEndpointType, RouteTableIds (VpcEndpointRouteTableIdList, wire name RouteTableId), SubnetIds, SecurityGroupIds, PolicyDocument, TagSpecifications; output VpcEndpoint (+ ClientToken); the operation declares no modeled errors, so a well-formed Gateway-endpoint request (a ServiceName such as com.amazonaws.<region>.s3 together with RouteTableIds) is accepted with HTTP 200 and returns a VpcEndpoint whose VpcEndpointId, VpcEndpointType=Gateway, State (pending/available) and RouteTableIds echo the associated route tables; a malformed one is refused with an EC2 query-protocol error code, never with a not-implemented status.
Emulator: the call answers NotImplemented with the message 'operation CreateVpcEndpoint with route table associations is scaffolded but not implemented in this emulator' — the handler exists but its route-table-association branch is a stub, so a request that merely includes RouteTableIds is refused outright. The message text itself is emulator-authored and corresponds to no AWS error.
Why: no S3 gateway endpoint can be provisioned, so any client (and both checks in this group) that builds the VPC plumbing an EMR cluster needs before submitting a Spark/Hive step fails during setup and never reaches the EMR or Glue behaviour it is exercising.
Fix: the ec2 crate's CreateVpcEndpoint handler — implement the gateway-endpoint route-table-association path and record the associations in the VPC-endpoint state so DescribeVpcEndpoints reports them.
Traces: emr__glue.emr-spark-step-on-an-unregistered-glue-table-fails, emr__glue.on-cluster-add-partition-lands-in-the-glue-catalog.

Note on attribution: the failing operation belongs to EC2, but the suite lists both checks under the emr__glue heading, so this issue carries that service token.

## Evidence
Replay of the simparity-behavioral-aws-v1 behavioral suite against the emulator built from this checkout (service heading emr__glue, 0/2 passed).
Both checks abort on the same call: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateVpcEndpoint operation: operation CreateVpcEndpoint with route table associations is scaffolded but not implemented in this emulator — expected HTTP 200 with a VpcEndpoint whose RouteTableIds echo the request.

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Run the simparity-behavioral-aws-v1 suite and inspect emr__glue.emr-spark-step-on-an-unregistered-glue-table-fails and emr__glue.on-cluster-add-partition-lands-in-the-glue-catalog.
3. Minimal manual repro: ec2.create_vpc(...), ec2.create_route_table(...), then ec2.create_vpc_endpoint(VpcId=<vpc>, ServiceName='com.amazonaws.<region>.s3', VpcEndpointType='Gateway', RouteTableIds=[<rtb>]) — expect 200 with VpcEndpoint.RouteTableIds=[<rtb>], observe NotImplemented.
