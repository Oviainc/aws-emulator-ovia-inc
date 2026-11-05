Issue-Source: trace-sweep-divergence
Issue-Trace: rds-create-db-instance-catalog-triple
Issue-Step: task#14
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-create-db-instance-catalog-triple rds-describe-instance-case-insensitive rds-maintenance-and-account-attributes rds-refarch-aur-create-cluster-production-settings-refusals
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.Endpoint.Port
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.Endpoint.Port
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.Endpoint.Address
Issue-Key: rds|ApplyPendingMaintenanceAction|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: rds|ApplyPendingMaintenanceAction|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|ApplyPendingMaintenanceAction|4xx->4xx|status
Issue-Key: rds|CreateDBCluster|4xx->5xx|$.ErrorResponse.Error.Code
Issue-Key: rds|CreateDBCluster|4xx->5xx|$.ErrorResponse.Error.Message
Issue-Key: rds|CreateDBCluster|4xx->5xx|$.ErrorResponse.Error.Type
Issue-Key: rds|CreateDBCluster|4xx->5xx|status
Issue-Related: 20261022T162132Z-rds-describedbinstances-omits-modeled-members-and-reports-wrong.md
Issue-Related: 20261029T214632Z-rds-omits-vpc-subnet-and-availability-zone-attributes.md
Issue-Related: 20261030T001542Z-rds-db-subnet-group-and-db-instance-projections.md
Issue-Commit: 88f80dc26c3af7497f4d732466c4db82370b4d91

# rds: DB instance envelopes report emulator defaults instead of request/engine-derived values, and two refusals diverge (5 causes)

> _Note: this issue was identified against the code base at commit `88f80dc26c3af7497f4d732466c4db82370b4d91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBInstances

## Description
This issue bundles 5 net-new root causes in the rds service, all from wire-corpus clusters that the replay marked neither stub-blocked nor OWNED by an open issue. Three concern the DBInstance response envelope (members the emulator never serializes, and defaults it invents rather than deriving from the request / the engine catalog); two concern refusal paths that answer the wrong status, error code and message. A fixer must close all FIVE causes below before submitting.

=== Cause 1 of 5: DescribeDBInstances after a create that named an explicit DB subnet group reports the wrong instance class, no subnet-group VpcId and the internal listener's port ===
AWS: Smithy: DescribeDBInstances output DBInstanceMessage$DBInstances -> DBInstance. DBInstance$DBInstanceClass echoes the compute class the create actually applied; DBInstance$DBSubnetGroup is the DBSubnetGroup shape whose VpcId member "Provides the VpcId of the DB subnet group"; DBInstance$Endpoint is the Endpoint shape whose Port member "Specifies the port that the database engine is listening on" - when CreateDBInstanceMessage$Port is omitted RDS applies the engine default (3306 for mysql/mariadb/aurora-mysql, 5432 for postgres/aurora-postgresql) and publishes that stored value.
Emulator: DBInstanceClass comes back as a different class from the one the create named (the run reports db.t2.micro where the request/recording used db.t4g.micro), DBSubnetGroup.VpcId is absent even though the instance was placed in a named subnet group, and Endpoint.Port is whichever ephemeral port the internal engine listener happened to bind (38159 in this run) rather than the requested or engine-default port.
Why: a client cannot read back the class it paid for, cannot discover which VPC the instance sits in (so it cannot author security-group or route rules), and any connection string, CloudFormation output or firewall rule pinned to the published port points at a port that changes every boot.
Fix: the DescribeDBInstances/DBInstance serializer and the CreateDBInstance handler's class and endpoint-port bookkeeping in the rds crate's state.
Related open issues that do NOT carry this fixture#step: 20261029T214632Z-rds-omits-vpc-subnet-and-availability-zone-attributes.md (class + subnet-group VpcId, same fixture step but a narrower cluster key), and 20261105T131136Z-rds-10-net-new-causes-the-engine-data.md Cause 1 (published port, behavioral check ids only).
Traces: rds-create-db-instance-catalog-triple#task#14.

=== Cause 2 of 5: the DeleteDBInstance response's DBInstance envelope drops the subnet group, the tag list and the VPC/HTTP-endpoint flags, and carries invented storage defaults ===
AWS: Smithy: DeleteDBInstance output DeleteDBInstanceResult$DBInstance, the same DBInstance shape Describe returns - the delete response is a full snapshot of the instance as it enters deleting. The recording carries DBSubnetGroup fully populated (VpcId, DBSubnetGroupDescription as given at create, and the Subnets list with each Subnet's SubnetIdentifier/SubnetStatus/SubnetOutpost/SubnetAvailabilityZone), the instance's TagList as a Tag list (the tags supplied at create), VPCNetworkingEnabled true and HttpEndpointEnabled false for a VPC instance with no Data API, StorageType as the type the create resolved (gp2 here), and StorageThroughput 0 because provisioned throughput exists only for gp3. Recorded behavior only for CACertificateIdentifier: AWS reports it empty on the delete envelope.
Emulator: VPCNetworkingEnabled, HttpEndpointEnabled and the whole DBSubnetGroup subtree (VpcId, Subnets, DBSubnetGroupDescription) are absent; TagList is serialized as an empty string instead of a Tag list, so tags set at create vanish; StorageType is reported gp3 with StorageThroughput 125 where the resolved type was gp2/0; CACertificateIdentifier is populated (rds-ca-rsa2048-g1) where AWS reports empty; DBInstanceClass and Endpoint.Port diverge as in Cause 1. The status is 200 in both.
Why: a client that uses the delete response as its last record of the instance (for teardown bookkeeping, tag-based cost attribution, or to confirm which VPC/subnets to clean up next) loses all of it, and reads a storage configuration the instance never had.
Fix: the DeleteDBInstance handler's response serializer - it must emit the same fully populated DBInstance envelope as Describe, and the storage-default resolution (StorageType/StorageThroughput) must come from what the create resolved, not from a hardcoded gp3/125.
Traces: rds-create-db-instance-catalog-triple#task#15.

=== Cause 3 of 5: a DB instance created without an explicit engine version / subnet group gets a stale default version, stale derived group names, gp3 storage and no default subnet group ===
AWS: Smithy: CreateDBInstanceMessage$EngineVersion is optional; when it is omitted RDS assigns the engine's current default version and DERIVES the instance's DBParameterGroups$DBParameterGroupName as default.<engine><majorVersion> and OptionGroupMemberships$OptionGroupName as default:<engine>-<majorVersion> from that assigned version (the recording shows the postgres default-latest of its day, 18.3, with default.postgres18 and default:postgres-18). When DBSubnetGroupName is omitted the instance joins the account's default subnet group and DBInstance$DBSubnetGroup is reported with DBSubnetGroupName/Description "default", SubnetGroupStatus "Complete", a VpcId and the Subnets list. DBInstance$VPCNetworkingEnabled is true and HttpEndpointEnabled false for such an instance; StorageType is the resolved default (gp2 in the recording) and StorageThroughput is 0 for any non-gp3 type. DBInstance$Endpoint$Address is an RDS endpoint DNS name (<identifier>-<token>.<region>.rds.amazonaws.com form; the corpus asserts the shape, not a literal host) and Endpoint$Port the engine default (5432 for postgres).
Emulator: the postgres default engine version is pinned to a stale minor (17.2 where the recording's default is 18.3), and because the group names are derived from it the parameter group and option group are stale too (default.postgres17, default:postgres-17); DBSubnetGroup is omitted entirely for an instance created without one; VPCNetworkingEnabled and HttpEndpointEnabled are absent; StorageType is gp3 with StorageThroughput 125 instead of the resolved gp2/0; Endpoint.Address is the loopback literal 127.0.0.1, which does not match the RDS endpoint-name shape; Endpoint.Port is the listener's ephemeral port (33757).
Why: a client that applies a parameter or option group by the derived default name addresses a group that does not match the engine version it actually got; a client that reads DBSubnetGroup to discover placement sees nothing; a client that resolves Endpoint.Address through DNS (rather than connecting to a loopback) cannot reach the instance at all. The requirement is that each of these be derived from the assigned engine version / resolved storage type / the subnet group in state, not that any particular version number appear.
Fix: the rds crate's engine-version catalog and default resolution, the parameter-/option-group name derivation, the default-subnet-group association in state, and the DBInstance serializer (VPCNetworkingEnabled, HttpEndpointEnabled, DBSubnetGroup, StorageType/StorageThroughput, Endpoint).
Traces: rds-describe-instance-case-insensitive#task#2.

=== Cause 4 of 5: ApplyPendingMaintenanceAction on an unresolvable resource ARN answers 404 ResourceNotFoundFault with emulator prose instead of 400 InvalidParameterValue ===
AWS: Smithy: ApplyPendingMaintenanceActionMessage$ResourceIdentifier is required and is the ARN of the DB instance/cluster. The operation models ResourceNotFoundFault (httpError 404, awsQueryError code ResourceNotFoundFault), but AWS classifies an ARN that resolves to nothing as a parameter-value error on that member: HTTP 400, Error.Type Sender, Error.Code InvalidParameterValue, Error.Message "Unknown resource: " followed by the ResourceIdentifier exactly as the request supplied it. The fixed text is "Unknown resource: "; the ARN after it is the request's own value (arn:aws:rds:<region>:<account>:db:<identifier>), not a value of this recording.
Emulator: answers HTTP 404 with Error.Code "ResourceNotFoundFault" and Error.Message "Resource not found: <bare identifier>" - the wrong status class, the wrong error code, and an emulator-authored sentence that both differs from AWS's fixed text and echoes only the last ARN segment instead of the ARN the client passed.
Why: an SDK retry/branching policy keyed on 4xx class and error code treats a malformed/stale ARN as a transient not-found (often retried or routed to a "wait for the resource" path) instead of surfacing the invalid parameter the caller must fix, and a client that logs the returned message loses the ARN it actually sent.
Fix: the ApplyPendingMaintenanceAction handler's ResourceIdentifier validation and error classification in the rds crate.
Traces: rds-maintenance-and-account-attributes#task#3.

=== Cause 5 of 5: CreateDBCluster with both MasterUserPassword and ManageMasterUserPassword answers 501 NotImplemented (Receiver) instead of the 400 mutual-exclusion refusal ===
AWS: Smithy: CreateDBClusterMessage$ManageMasterUserPassword documents the constraint "Can't manage the master user password with Amazon Web Services Secrets Manager if MasterUserPassword is specified." The request is refused before any provisioning or Secrets Manager work: HTTP 400, Error.Type Sender, Error.Code InvalidParameterValue, Error.Message "MasterUserPassword and ManageMasterUserPassword are mutually exclusive. Specify only one of these parameters." - that message is entirely fixed text with no variable part.
Emulator: answers HTTP 501 with Error.Type "Receiver", Error.Code "NotImplemented" and the emulator-authored sentence "CreateDBCluster$ManageMasterUserPassword is not implemented in this emulator: it has RDS generate the master user password in Amazon Web Services Secrets Manager and report that secret as MasterUserSecret, and this service mints no such secret. Name MasterUserPassword instead." The unsupported-feature refusal is reached BEFORE the input validation, so an invalid request is reported as a server-side gap.
Why: Error.Type Receiver with a 5xx status is a server fault - SDK retry policies retry it until the attempt budget is exhausted, and the caller never learns that its own parameter combination is illegal. The negative path of every "exactly one password source" test is unreachable.
Fix: CreateDBCluster's input validation ordering in the rds crate - the MasterUserPassword/ManageMasterUserPassword mutual-exclusion check must run before the unsupported-feature refusal, and must render AWS's InvalidParameterValue envelope.
Related open issue that does NOT carry this fixture#step: 20261105T131136Z-rds-secretsmanager-modifydbinstance-managemasteruserpassword-true-never-publishes-a.md (ModifyDBInstance, no refusal-ordering claim).
Traces: rds-refarch-aur-create-cluster-production-settings-refusals#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, score 43.6%). Five clusters reported with stub_path="" and owned_by=[]:

Cause 1 - rds-create-db-instance-catalog-triple#task#14 DescribeDBInstances, status 200 expected and 200 actual:
  DBInstance.DBInstanceClass: value_mismatch (expected 'db.t4g.micro', got 'db.t2.micro')
  DBInstance.DBSubnetGroup.VpcId: missing (expected a vpc id, got None)
  DBInstance.Endpoint.Port: value_mismatch (expected '3306', got '38159')

Cause 2 - rds-create-db-instance-catalog-triple#task#15 DeleteDBInstance, status 200/200:
  DBInstance.VPCNetworkingEnabled: missing (expected 'true', got None)
  DBInstance.HttpEndpointEnabled: missing (expected 'false', got None)
  DBInstance.DBSubnetGroup.VpcId / .Subnets / .DBSubnetGroupDescription: missing (expected the populated subnet group, got None)
  DBInstance.TagList: type_mismatch (expected a Tag list, got '')
  DBInstance.StorageType: value_mismatch (expected 'gp2', got 'gp3')
  DBInstance.StorageThroughput: value_mismatch (expected '0', got '125')
  DBInstance.CACertificateIdentifier: value_mismatch (expected '', got 'rds-ca-rsa2048-g1')
  DBInstance.DBInstanceClass: value_mismatch (expected 'db.t4g.micro', got 'db.t2.micro')
  DBInstance.Endpoint.Port: value_mismatch (expected '3306', got '38159')

Cause 3 - rds-describe-instance-case-insensitive#task#2 DescribeDBInstances, status 200/200:
  DBInstance.EngineVersion: value_mismatch (expected '18.3', got '17.2')
  DBInstance.DBParameterGroups.DBParameterGroup.DBParameterGroupName: value_mismatch (expected 'default.postgres18', got 'default.postgres17')
  DBInstance.OptionGroupMemberships.OptionGroupMembership.OptionGroupName: value_mismatch (expected 'default:postgres-18', got 'default:postgres-17')
  DBInstance.DBSubnetGroup: missing (expected {DBSubnetGroupName 'default', DBSubnetGroupDescription 'default', SubnetGroupStatus 'Complete', VpcId, Subnets}, got None)
  DBInstance.VPCNetworkingEnabled: missing (expected 'true'); DBInstance.HttpEndpointEnabled: missing (expected 'false')
  DBInstance.StorageType: value_mismatch (expected 'gp2', got 'gp3'); StorageThroughput: expected '0', got '125'
  DBInstance.Endpoint.Address: value_mismatch (expected the asserted RDS endpoint-name shape, got '127.0.0.1')
  DBInstance.Endpoint.Port: value_mismatch (expected '5432', got '33757')

Cause 4 - rds-maintenance-and-account-attributes#task#3 ApplyPendingMaintenanceAction:
  status: expected 400, got 404
  ErrorResponse.Error.Code: expected 'InvalidParameterValue', got 'ResourceNotFoundFault'
  ErrorResponse.Error.Message: expected 'Unknown resource: arn:aws:rds:<region>:<account>:db:<identifier>' (fixed text 'Unknown resource: ' + the request's ResourceIdentifier), got 'Resource not found: <identifier>'

Cause 5 - rds-refarch-aur-create-cluster-production-settings-refusals#task#6 CreateDBCluster:
  status: expected 400, got 501
  ErrorResponse.Error.Type: expected 'Sender', got 'Receiver'
  ErrorResponse.Error.Code: expected 'InvalidParameterValue', got 'NotImplemented'
  ErrorResponse.Error.Message: expected 'MasterUserPassword and ManageMasterUserPassword are mutually exclusive. Specify only one of these parameters.', got 'CreateDBCluster$ManageMasterUserPassword is not implemented in this emulator: ... Name MasterUserPassword instead.'

(vpc-/subnet- ids, the 38159/33757 listener ports and the generated host above are this run's values; the requirements are the members' presence, the derived names and the engine-default/requested port.)

## Steps to Reproduce
1. git clone the repository and boot the emulator built from that checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1+2: re-replay rds-create-db-instance-catalog-triple#task#14 and #task#15 - create a DB subnet group in a VPC, CreateDBInstance(DBInstanceClass=db.t4g.micro, Engine=mysql, DBSubnetGroupName=<that group>), then DescribeDBInstances and DeleteDBInstance and diff DBInstanceClass, DBSubnetGroup.*, TagList, VPCNetworkingEnabled, HttpEndpointEnabled, StorageType/StorageThroughput, CACertificateIdentifier and Endpoint.Port.
4. Cause 3: re-replay rds-describe-instance-case-insensitive#task#2 - CreateDBInstance(Engine=postgres) with no EngineVersion and no DBSubnetGroupName, then DescribeDBInstances and diff EngineVersion, DBParameterGroups, OptionGroupMemberships, DBSubnetGroup, StorageType/StorageThroughput and Endpoint.Address/Port.
5. Cause 4: re-replay rds-maintenance-and-account-attributes#task#3 - ApplyPendingMaintenanceAction(ResourceIdentifier=arn of a db that does not exist, ApplyAction/OptInType set) and read the HTTP status, Error.Code and Error.Message.
6. Cause 5: re-replay rds-refarch-aur-create-cluster-production-settings-refusals#task#6 - CreateDBCluster with BOTH MasterUserPassword and ManageMasterUserPassword=true and read the HTTP status, Error.Type, Error.Code and Error.Message.
