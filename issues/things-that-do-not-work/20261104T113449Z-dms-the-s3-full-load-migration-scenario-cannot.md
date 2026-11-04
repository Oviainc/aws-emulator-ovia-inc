Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dms.s3-full-load-migration
Issue-Shape-Id: com.amazonaws.databasemigrationservice#AmazonDMSv20160101
Issue-Key: dms|CreateVpcEndpoint|NotImplemented
Issue-Commit: 55cf77d522c1895c18e1a6aab58b83f0cfc3c7c5

# dms: the S3 full-load migration scenario cannot be set up — EC2 CreateVpcEndpoint refuses any request carrying RouteTableIds with NotImplemented

> _Note: this issue was identified against the code base at commit `55cf77d522c1895c18e1a6aab58b83f0cfc3c7c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcEndpoint

## Description
One net-new root cause, grounded in the dms behavioral check below.

Cause 1 of 1: creating the Gateway VPC endpoint the DMS replication instance needs in order to reach S3 without an internet path is refused with HTTP 501 / NotImplemented as soon as the request names RouteTableIds, so the whole S3-source-to-S3-target full-load migration scenario never gets past VPC setup.

AWS: Smithy com.amazonaws.ec2#CreateVpcEndpoint (model services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json) takes VpcId (required, clientOptional), ServiceName, VpcEndpointType (enum, 'Gateway' here), RouteTableIds (list, xmlName RouteTableId), SubnetIds, SecurityGroupIds, PolicyDocument, DnsOptions, PrivateDnsEnabled, TagSpecifications, and returns CreateVpcEndpointResult carrying VpcEndpoint (xmlName vpcEndpoint) plus ClientToken. The operation models NO errors at all: RouteTableIds is an ordinary modelled input member of the accepted request, and a Gateway endpoint associated with one or more route tables is the normal, documented way to attach an S3 gateway endpoint to a VPC. Real AWS answers HTTP 200 with a vpcEndpoint whose vpcEndpointType is 'Gateway', whose state becomes 'available', whose routeTableIdSet lists exactly the requested route tables, and whose serviceName is the regional S3 service name. There is no error class under which AWS can refuse this request shape, and certainly not a 5xx.

Emulator: the call is answered with HTTP 501 and error code NotImplemented, message 'operation CreateVpcEndpoint with route table associations is scaffolded but not implemented in this emulator'. Fixed text of that message: 'operation ', ' with route table associations is scaffolded but not implemented in this emulator'; the only variable part is the operation name taken from the request ('CreateVpcEndpoint'). The refusal is conditioned on the presence of the RouteTableIds member, i.e. the handler serves the no-route-table shapes and bails out on the gateway shape. Nothing about the request is account-specific or per-run: the VPC, subnets and route table are all created moments earlier in the same check.

Why: a client building the standard private DMS estate (VPC, two subnets in two AZs, S3 gateway endpoint on the main route table, replication subnet group, replication instance, S3 source and target endpoints, full-load task) is stopped at step one with a 5xx that no SDK models for this operation. Botocore raises ClientError(NotImplemented), which application code written against AWS has no handler for, and the error class is wrong besides — a 501 reads as 'the service is broken' rather than a client-correctable condition. Everything downstream of the endpoint (replication subnet group creation, instance availability, task readiness, the full-load run, the migrated LOAD00000001.csv in the target bucket, and DescribeTableStatistics reporting FullLoadRows/TableState) is consequently never exercised, so the entire DMS data-plane migration path is ungradeable through this emulator.

Fix: the EC2 CreateVpcEndpoint handler — the Gateway branch that sees RouteTableIds must build and persist the endpoint, associate it with the named route tables, and project it into CreateVpcEndpointResult.VpcEndpoint (state 'available', vpcEndpointType 'Gateway', populated routeTableIdSet), instead of falling through to the NotImplemented path. The associated route-table state belongs with the VPC-endpoint state this crate already keeps for the non-gateway shapes.

Related already-open issues describe the same EC2 refusal from other services' corpora (e.g. 20261104T062058Z-ec2-createvpcendpoint-answers-501-notimplemented-for-a-gateway, 20261104T044826Z-ec2-lambda-s3-createvpcendpoint-with-route-table-associations-is-refused-notimplemented, 20261104T055008Z-ec2-s3-ssm-createvpcendpoint-with-route-tables-and) but none of them carries this dms check's replay key, so re-replaying them would not confirm the DMS migration path is unblocked.

Traces: dms.s3-full-load-migration.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, emulator up (1/3 dms checks passed).

[error] dms.s3-full-load-migration:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateVpcEndpoint operation: operation CreateVpcEndpoint with route table associations is scaffolded but not implemented in this emulator

Expected (AWS): HTTP 200, CreateVpcEndpointResult.VpcEndpoint with VpcEndpointType 'Gateway', State 'available', RouteTableIds == the single main route table of the just-created VPC, ServiceName 'com.amazonaws.<region>.s3'. CreateVpcEndpoint models no errors in the Smithy model, so no refusal is permissible for this request shape.
Actual (emulator): HTTP 501, Error.Code 'NotImplemented', Error.Message 'operation CreateVpcEndpoint with route table associations is scaffolded but not implemented in this emulator'.

The failing call is issued from the check's VPC setup:
  ec2.create_vpc_endpoint(VpcId=vpc_id, ServiceName=f'com.amazonaws.{region}.s3', VpcEndpointType='Gateway', RouteTableIds=[main_rt])
where main_rt is the Main association from DescribeRouteTables on the check's own VPC. The refusal aborts the check before any DMS call is made, so CreateReplicationSubnetGroup, CreateReplicationInstance, CreateEndpoint (s3 source/target), CreateReplicationTask, StartReplicationTask, the FULL_LOAD_ONLY_FINISHED StopReason, ReplicationTaskStats and DescribeTableStatistics are all left unverified.

Model consulted: smithy_query query=operation operation=CreateVpcEndpoint on services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json — input members include RouteTableIds (target VpcEndpointRouteTableIdList, xmlName RouteTableId), errors: [].

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the behavioral suite simparity-behavioral-aws-v1 and run check dms.s3-full-load-migration.
3. The check creates a VPC (10.31.0.0/16), two subnets in two available AZs, reads the VPC's main route table via DescribeRouteTables, and then calls EC2 CreateVpcEndpoint with VpcEndpointType='Gateway', ServiceName='com.amazonaws.<region>.s3' and RouteTableIds=[<main route table id>].
4. Observe HTTP 501 / NotImplemented with message 'operation CreateVpcEndpoint with route table associations is scaffolded but not implemented in this emulator' where AWS answers HTTP 200 with an 'available' Gateway vpcEndpoint whose routeTableIdSet holds the requested route table.
5. Minimal repro without DMS: CreateVpc, DescribeRouteTables for that VPC, then CreateVpcEndpoint with VpcEndpointType='Gateway' and RouteTableIds=[main]. Dropping RouteTableIds from the same request changes the outcome, which localises the refusal to the route-table-association branch of the handler.
