Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-instance-core-lifecycle
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-instance-core-lifecycle rds-describe-instance-case-insensitive
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.EngineVersion
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.HttpEndpointEnabled
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.PendingModifiedValues
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.StorageThroughput
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.StorageType
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.VPCNetworkingEnabled
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.VpcSecurityGroups
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.DBParameterGroups.DBParameterGroup.DBParameterGroupName
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.DBSubnetGroup
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.Endpoint.HostedZoneId
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.OptionGroupMemberships.OptionGroupMembership.OptionGroupName
Issue-Commit: 41eca1b5094053f7eb75d5bb9c5bde3b8db45df6

# RDS DescribeDBInstances omits modeled members and reports wrong storage and engine defaults

> _Note: this issue was identified against the code base at commit `41eca1b5094053f7eb75d5bb9c5bde3b8db45df6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBInstances

## Description
The DBInstance envelope DescribeDBInstances serves is incomplete and carries the wrong defaults: members AWS always emits are absent, collection members are serialised as an empty scalar instead of a populated structure, and the storage/engine defaults the emulator applies when CreateDBInstance omits them differ from AWS's. Both clusters answer 200, so the fault is the DBInstance projection and the create-time defaulting behind it. Two causes.

Cause 1 of 2: the DBInstance envelope for a freshly created instance omits members and misreports storage defaults.
AWS: Smithy: DescribeDBInstances -> DBInstanceMessage {Marker, DBInstances: DBInstanceList of DBInstance}; DBInstance models StorageType, StorageThroughput, EngineVersion, VpcSecurityGroups: VpcSecurityGroupMembershipList and PendingModifiedValues: PendingModifiedValues. Recorded behavior only for the values and for the two members not in this checkout's vendored DBInstance shape: AWS emits VPCNetworkingEnabled 'true' and HttpEndpointEnabled 'false' on every instance, reports StorageThroughput '0' and StorageType 'gp2' for an instance created without an explicit storage type, resolves an unpinned engine version to 8.4.11, and emits a populated VpcSecurityGroups.VpcSecurityGroupMembership and a PendingModifiedValues with MasterUserPassword.
Emulator: VPCNetworkingEnabled and HttpEndpointEnabled are absent (got None); StorageThroughput is '125' and StorageType 'gp3' instead of '0'/'gp2'; EngineVersion resolves to 8.4.4 instead of 8.4.11; VpcSecurityGroups and PendingModifiedValues are serialised as the empty scalar '' instead of the structures AWS returns.
Why: an SDK deserialising DBInstance gets null for the two capability flags, an empty list for the security-group memberships the instance actually has, and a default storage type/throughput pair that does not match AWS - so a client that reads back what it created to confirm the applied configuration, or that compares the instance against a desired state, sees a different instance. The empty-scalar-for-collection form also changes the parsed type for VpcSecurityGroups and PendingModifiedValues.
Fix: the DBInstance serializer (the two absent members and the empty-collection emission) and the storage/engine-version defaulting in the CreateDBInstance handler whose result it projects.
Traces: rds-db-instance-core-lifecycle#task#2.

Cause 2 of 2: the same envelope also drops DBSubnetGroup and Endpoint.HostedZoneId and resolves the wrong default parameter/option group.
AWS: Smithy: DBInstance models DBSubnetGroup: DBSubnetGroup, Endpoint {Address, Port, HostedZoneId}, DBParameterGroups: DBParameterGroupStatusList and OptionGroupMemberships: OptionGroupMembershipList. Recorded behavior only for the values: for an instance in the default VPC AWS emits a DBSubnetGroup named 'default' with its Subnets and SubnetGroupStatus 'Complete', an Endpoint.HostedZoneId, DBParameterGroupName 'default.postgres18' and OptionGroupName 'default:postgres-18' for an instance whose engine version resolves to 18.3, plus VPCNetworkingEnabled/HttpEndpointEnabled and StorageType gp2 / StorageThroughput 0 as in cause 1.
Emulator: DBSubnetGroup is absent entirely, Endpoint.HostedZoneId is absent, VPCNetworkingEnabled and HttpEndpointEnabled are absent, VpcSecurityGroups is the empty scalar '', EngineVersion resolves to 17.2 instead of 18.3 and the derived DBParameterGroupName/OptionGroupName follow it to default.postgres17 / default:postgres-17, and StorageType/StorageThroughput are gp3/125.
Why: a client that needs the instance's subnet group or the endpoint's HostedZoneId - for example to create a DNS alias or to decide which VPC the instance sits in - finds nothing to read, and the parameter/option group names it would pass to ModifyDBInstance name the wrong engine major version.
Fix: the same DBInstance serializer and the engine-version resolution that the default parameter-group and option-group names are derived from.
Traces: rds-describe-instance-case-insensitive#task#2.

## Evidence
Both clusters replay expected_status 200 / actual_status 200. Cause 1: $...DBInstance.VPCNetworkingEnabled: missing (expected 'true', got None); HttpEndpointEnabled: missing (expected 'false', got None); StorageThroughput expected '0', got '125'; StorageType expected 'gp2', got 'gp3'; EngineVersion expected '8.4.11', got '8.4.4'; VpcSecurityGroups type_mismatch (expected {'VpcSecurityGroupMembership': {...'Status': 'active'}}, got ''); PendingModifiedValues type_mismatch (expected {'MasterUserPassword': ...}, got ''). Cause 2: DBSubnetGroup missing (expected the 'default' group with Subnets and SubnetGroupStatus 'Complete', got None); Endpoint.HostedZoneId missing; DBParameterGroups.DBParameterGroup.DBParameterGroupName expected 'default.postgres18', got 'default.postgres17'; OptionGroupMemberships.OptionGroupMembership.OptionGroupName expected 'default:postgres-18', got 'default:postgres-17'; EngineVersion expected '18.3', got '17.2'.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture rds-db-instance-core-lifecycle through step task#2 (CreateDBInstance then Action=DescribeDBInstances) and diff DescribeDBInstancesResult.DBInstances.DBInstance against the recording: VPCNetworkingEnabled/HttpEndpointEnabled absent, VpcSecurityGroups and PendingModifiedValues empty scalars, StorageType gp3 and StorageThroughput 125 instead of gp2/0, EngineVersion 8.4.4 instead of 8.4.11.
3. Replay rds-describe-instance-case-insensitive#task#2 and observe the missing DBSubnetGroup and Endpoint.HostedZoneId and the default.postgres17 / default:postgres-17 group names.
