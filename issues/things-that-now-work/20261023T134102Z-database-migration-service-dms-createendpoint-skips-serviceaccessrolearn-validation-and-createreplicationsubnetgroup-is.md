Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dms.endpoint-rejects-unusable-role dms.subnet-group-requires-two-azs
Issue-Shape-Id: com.amazonaws.databasemigrationservice#AmazonDMSv20160101
Issue-Key: dms.endpoint-rejects-unusable-role|_error#15|AssertionError|raise AssertionError("expected the call to be rejected, but it succeeded")
Issue-Key: dms.subnet-group-requires-two-azs|run#16|AssertionError|assert code == 'ReplicationSubnetGroupDoesNotCoverEnoughAZs', (
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# DMS: CreateEndpoint skips ServiceAccessRoleArn validation and CreateReplicationSubnetGroup is an unimplemented 501 stub (2 causes)

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEndpoint

## Description
Two distinct root causes in the DMS emulator, both grounded in failing behavioral checks. No existing open issue names either operation (list_existing_issues for 'dms'/'databasemigration' returns nothing).

=== Cause 1 of 2: CreateEndpoint accepts an S3 endpoint whose ServiceAccessRoleArn does not resolve to an assumable role ===
Failing check: database-migration-service.endpoint-rejects-unusable-role

AWS: CreateEndpoint validates the service access role at create time and refuses the request; it does not defer the check to TestConnection. Smithy (com-amazonaws-databasemigrationservice-amazondmsv20160101): CreateEndpoint models AccessDeniedFault (HTTP 400, documented "DMS was denied access to the endpoint. Check that the role is correctly configured.") and S3AccessDeniedFault; input member ServiceAccessRoleArn is documented as "The Amazon Resource Name (ARN) for the service access role that you want to use to create the endpoint. The role must allow the iam:PassRole action." The recorded AWS response for a well-formed, account-correct ARN naming a role that does not exist is HTTP 400 with error code AccessDeniedFault (recorded message, as an example only: "The IAM Role <arn> is not configured properly.AccessDenied"). Because the create is refused, nothing is persisted: a subsequent DescribeEndpoints filtered on that endpoint-id answers HTTP 400 ResourceNotFoundFault rather than an empty list.

Emulator: the call SUCCEEDS. The check's _error() helper raised "expected the call to be rejected, but it succeeded" at CreateEndpoint. services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/src/ops/create_endpoint.rs validates only EndpointIdentifier/ResourceIdentifier shape and uniqueness, then stores input.service_access_role_arn verbatim into EndpointRecord (line ~362) with no check that the ARN resolves to an assumable role. The refused endpoint is therefore also left behind in state, so the absence assertion (ResourceNotFoundFault from DescribeEndpoints) cannot hold either.

Why: a client that relies on DMS rejecting a mis-specified role up front sees a green CreateEndpoint against the emulator and ships a broken configuration. Error-handling paths for AccessDeniedFault are never exercised, and the emulator's endpoint inventory contains endpoints real AWS would never have created, so later DescribeEndpoints/CreateReplicationTask flows diverge too.

Fix: CreateEndpoint's validation/handler — services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/src/ops/create_endpoint.rs (handle(), before the record is inserted, around lines 270-370): resolve ServiceAccessRoleArn (top-level and the per-engine settings copy, e.g. S3Settings.ServiceAccessRoleArn) against the emulator's IAM state and return AccessDeniedFault with HTTP 400 when it does not name an existing assumable role, persisting nothing.

Traces: database-migration-service.endpoint-rejects-unusable-role

=== Cause 2 of 2: CreateReplicationSubnetGroup answers 501 NotImplemented; the two-availability-zone rule is absent ===
Failing check: database-migration-service.subnet-group-requires-two-azs

AWS: CreateReplicationSubnetGroup requires the supplied subnets to span at least two availability zones and refuses otherwise. Smithy: CreateReplicationSubnetGroup models ReplicationSubnetGroupDoesNotCoverEnoughAZs (HTTP 400, "The replication subnet group does not cover enough Availability Zones (AZs). Edit the replication subnet group and add more AZs."), and the operation doc states "The VPC needs to have at least one subnet in at least two availability zones in the Amazon Web Services Region, otherwise the service will throw a ReplicationSubnetGroupDoesNotCoverEnoughAZs exception." Note SubnetIds is a plain SubnetIdentifierList with no modeled minimum, so this is a business rule the handler must enforce, not a model constraint. Recorded AWS behavior: HTTP 400 / ReplicationSubnetGroupDoesNotCoverEnoughAZs for a single-zone group (recorded message, as an example, is built from the request and the region's zone list: "Replication Subnet Group doesn't meet availability zone coverage requirement. ... Current AZ coverage: 1. Valid availability zones are: [...]"); the refused create persists nothing, so DeleteReplicationSubnetGroup for that identifier answers HTTP 400 ResourceNotFoundFault; and the identical request with a second subnet in a second zone succeeds, returning a ReplicationSubnetGroup with SubnetGroupStatus "Complete", the VpcId, and both subnets with their SubnetAvailabilityZone names.

Emulator: the request is rejected with error code 'NotImplemented' (HTTP 501). AssertionError: "a single-zone subnet group was rejected as 'NotImplemented', expected 'ReplicationSubnetGroupDoesNotCoverEnoughAZs'". The handler is still the generated scaffold stub: services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/src/ops/create_replication_subnet_group.rs carries the SCAFFOLDED, NOT IMPLEMENTED marker and its handle() ends in Err(crate::scaffolded::Scaffolded::not_implemented("CreateReplicationSubnetGroup")) (line 182). Nothing about subnet groups is modeled in state, so neither the rejection nor the positive control can be answered.

Why: a client cannot create or validate replication subnet groups at all against the emulator, and a 501 with code NotImplemented is not a modeled DMS error — SDK error handling for ReplicationSubnetGroupDoesNotCoverEnoughAZs/ResourceNotFoundFault never runs, and the permissive/explosive failure mode masks the AZ-coverage requirement that would surface in production when a replication instance is placed in the group.

Fix: implement the operation in its own handler file — services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/src/ops/create_replication_subnet_group.rs (handle(), replacing the not_implemented Err at line 182) plus subnet-group records in src/state.rs: resolve the SubnetIds against EC2 state, count distinct availability zones, return ReplicationSubnetGroupDoesNotCoverEnoughAZs (HTTP 400) and persist nothing when coverage is below 2, otherwise store and return the group with SubnetGroupStatus Complete, the derived VpcId and each subnet's availability zone. DeleteReplicationSubnetGroup must answer ResourceNotFoundFault for an identifier that was never stored. Delete the SCAFFOLDED marker line once done.

Traces: database-migration-service.subnet-group-requires-two-azs

## Evidence
Suite: database-migration-service 0/3 (0%), 3 failed.

[mismatch] database-migration-service.endpoint-rejects-unusable-role
  check_database_migration_service_endpoint_rejects_unusable_role.py line 98 -> _error(lambda: dms.create_endpoint(EndpointIdentifier=rejected_id, EndpointType='target', EngineName='s3', S3Settings=dict(settings, ServiceAccessRoleArn=absent_role_arn)))
  AssertionError: expected the call to be rejected, but it succeeded
  Expected: HTTP 400, Error.Code == 'AccessDeniedFault' (recorded message example: "The IAM Role <role-arn>-absent is not configured properly.AccessDenied"); then DescribeEndpoints(Filters=[endpoint-id=<id>]) -> HTTP 400 ResourceNotFoundFault.
  Actual: HTTP 200 with a created Endpoint; the endpoint is persisted in emulator state.

[mismatch] database-migration-service.subnet-group-requires-two-azs
  check_database_migration_service_subnet_group_requires_two_azs.py line 95
  AssertionError: a single-zone subnet group was rejected as 'NotImplemented', expected 'ReplicationSubnetGroupDoesNotCoverEnoughAZs'
  Expected: CreateReplicationSubnetGroup(SubnetIds=[one subnet in one AZ]) -> HTTP 400 ReplicationSubnetGroupDoesNotCoverEnoughAZs; DeleteReplicationSubnetGroup(<same id>) -> HTTP 400 ResourceNotFoundFault; then the same request with a second subnet in a second AZ -> HTTP 200 with SubnetGroupStatus 'Complete', the VpcId, and both subnets with their SubnetAvailabilityZone.Name.
  Actual: HTTP 501, Error.Code 'NotImplemented' (scaffold stub at src/ops/create_replication_subnet_group.rs line 182, SCAFFOLDED marker at line 4).

Not filed here: database-migration-service.s3-full-load-migration, which the report marks ALREADY FILED (blocked by unimplemented EC2 DescribeRouteTables).

## Steps to Reproduce
Cause 1 — CreateEndpoint role validation:
1. Create an S3 bucket and an IAM role trusted by dms.amazonaws.com with s3:PutObject/s3:ListBucket on that bucket.
2. Build a well-formed, account-correct role ARN that names a role which does NOT exist (e.g. the created role's ARN with '-absent' appended).
3. Call CreateEndpoint(EndpointIdentifier=<new-id>, EndpointType='target', EngineName='s3', S3Settings={BucketName:<bucket>, BucketFolder:'out', ServiceAccessRoleArn:<absent-arn>}).
   Expect HTTP 400 AccessDeniedFault; the emulator returns HTTP 200 and creates the endpoint.
4. Call DescribeEndpoints(Filters=[{Name:'endpoint-id', Values:[<new-id>]}]). Expect HTTP 400 ResourceNotFoundFault (nothing persisted); the emulator returns the endpoint the refused create should not have stored.
5. Positive control: the identical request with the role that does exist must succeed, so the rejection is attributable to the role alone.

Cause 2 — CreateReplicationSubnetGroup AZ coverage:
1. Ensure the fixed-name IAM role 'dms-vpc-role' exists (trusted by dms.amazonaws.com, AmazonDMSVPCManagementRole attached).
2. Create a VPC (e.g. 10.32.0.0/16) and ONE subnet in the first available AZ.
3. Call CreateReplicationSubnetGroup(ReplicationSubnetGroupIdentifier=<id>, ReplicationSubnetGroupDescription=<desc>, SubnetIds=[<subnet>]).
   Expect HTTP 400 ReplicationSubnetGroupDoesNotCoverEnoughAZs; the emulator answers HTTP 501 NotImplemented.
4. Call DeleteReplicationSubnetGroup(ReplicationSubnetGroupIdentifier=<id>). Expect HTTP 400 ResourceNotFoundFault.
5. Create a second subnet in a second AZ in the same VPC and repeat step 3 with both subnet ids. Expect HTTP 200 with ReplicationSubnetGroup.SubnetGroupStatus == 'Complete', VpcId == the VPC, both subnets present, and their SubnetAvailabilityZone.Name covering both zones.
