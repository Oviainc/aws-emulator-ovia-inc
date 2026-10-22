Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-instance-core-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-instance-core-lifecycle rds-describe-instance-case-insensitive rds-delete-groups-in-use-by-instance rds-create-db-instance-catalog-triple
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.ActivityStreamStatus
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.AssociatedRoles
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.BackupTarget
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.CertificateDetails
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.CustomerOwnedIpEnabled
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.DBParameterGroups
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.DBSecurityGroups
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.DatabaseInsightsMode
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.DbInstancePort
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.DedicatedLogVolume
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.DomainMemberships
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.EngineLifecycleSupport
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.EngineVersion
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.HttpEndpointEnabled
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.IAMDatabaseAuthenticationEnabled
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.IsStorageConfigUpgradeAvailable
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.LicenseModel
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.MonitoringInterval
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.NetworkType
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.OptionGroupMemberships
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.PendingModifiedValues
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.PerformanceInsightsEnabled
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.ReadReplicaDBInstanceIdentifiers
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.StorageThroughput
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.StorageType
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.UpgradeRolloutOrder
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.VPCNetworkingEnabled
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.VpcSecurityGroups
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.CACertificateIdentifier
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.DBSubnetGroup
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.Endpoint.HostedZoneId
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.PreferredBackupWindow
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.PreferredMaintenanceWindow
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.AssociatedRoles
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.BackupTarget
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.CACertificateIdentifier
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.CertificateDetails
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.CustomerOwnedIpEnabled
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.DBSecurityGroups
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.DBSubnetGroup.DBSubnetGroupDescription
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.DBSubnetGroup.Subnets
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.DBSubnetGroup.VpcId
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.DatabaseInsightsMode
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.DbInstancePort
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.DedicatedLogVolume
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.DomainMemberships
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.EngineLifecycleSupport
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.EngineVersion
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.HttpEndpointEnabled
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.IAMDatabaseAuthenticationEnabled
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.LicenseModel
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.MonitoringInterval
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.NetworkType
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.OptionGroupMemberships
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.PendingModifiedValues
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.PerformanceInsightsEnabled
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.PreferredBackupWindow
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.PreferredMaintenanceWindow
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.ReadReplicaDBInstanceIdentifiers
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.StorageThroughput
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.StorageType
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.UpgradeRolloutOrder
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.VPCNetworkingEnabled
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.VpcSecurityGroups
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.AssociatedRoles
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.BackupTarget
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.CACertificateIdentifier
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.CustomerOwnedIpEnabled
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.DBParameterGroups
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.DBSecurityGroups
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.DBSubnetGroup.DBSubnetGroupDescription
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.DBSubnetGroup.Subnets
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.DBSubnetGroup.VpcId
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.DatabaseInsightsMode
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.DbInstancePort
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.DedicatedLogVolume
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.DomainMemberships
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.EngineLifecycleSupport
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.EngineVersion
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.HttpEndpointEnabled
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.IAMDatabaseAuthenticationEnabled
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.LicenseModel
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.MonitoringInterval
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.NetworkType
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.OptionGroupMemberships
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.PendingModifiedValues
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.PerformanceInsightsEnabled
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.PreferredBackupWindow
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.PreferredMaintenanceWindow
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.ReadReplicaDBInstanceIdentifiers
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.StorageThroughput
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.StorageType
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.TagList
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.UpgradeRolloutOrder
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.VPCNetworkingEnabled
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.VpcSecurityGroups
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.DBSubnetGroup.VpcId
Issue-Commit: 0b8884c4ab978d23b4fe746f795a1277a2a36df3

# RDS DBInstance envelopes omit most modeled DBInstance members

> _Note: this issue was identified against the code base at commit `0b8884c4ab978d23b4fe746f795a1277a2a36df3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBInstances

## Description
Five causes, one fault: the RDS DBInstance serializer projects only a small subset of the modeled DBInstance structure, and the nested DBSubnetGroup projection is truncated. The same gap shows up on every operation that returns a DBInstance (Smithy: CreateDBInstance / DescribeDBInstances / ModifyDBInstance / DeleteDBInstance all return the DBInstance shape).

Cause 1 of 5 — DescribeDBInstances on a provisioned instance. AWS: the DBInstance element carries the modeled members, including VPCNetworkingEnabled, AssociatedRoles, DBParameterGroups, DatabaseInsightsMode, StorageThroughput, HttpEndpointEnabled, CustomerOwnedIpEnabled, DedicatedLogVolume, ActivityStreamStatus, PerformanceInsightsEnabled, LicenseModel, PendingModifiedValues, EngineLifecycleSupport, NetworkType, OptionGroupMemberships, CertificateDetails, DbInstancePort, DomainMemberships, MonitoringInterval, ReadReplicaDBInstanceIdentifiers, StorageType, UpgradeRolloutOrder, BackupTarget, IAMDatabaseAuthenticationEnabled, IsStorageConfigUpgradeAvailable, and a VpcSecurityGroups list with a VpcSecurityGroupMembership entry. Emulator: all of those members are absent and VpcSecurityGroups is serialized as an empty scalar instead of a membership list. Why: SDK models deserialize these as None/empty, so client code that reads e.g. DBParameterGroups, LicenseModel, NetworkType or PendingModifiedValues sees nothing where AWS always returns a value. Fix: the DBInstance serializer/state projection shared by the instance operations. Traces: rds-db-instance-core-lifecycle#task#2.

Cause 2 of 5 — DescribeDBInstances resolved case-insensitively. Same missing member set, plus CACertificateIdentifier, PreferredBackupWindow, Endpoint.HostedZoneId and the whole DBSubnetGroup element absent. Fix: as cause 1 (plus the Endpoint and DBSubnetGroup sub-projections). Traces: rds-describe-instance-case-insensitive#task#2.

Cause 3 of 5 — CreateDBInstance response envelope. AWS returns the full DBInstance in CreateDBInstanceResult, including CACertificateIdentifier, DBSubnetGroup.VpcId, DBSubnetGroup.Subnets (each Subnet with SubnetIdentifier/SubnetStatus/SubnetAvailabilityZone) and DBSubnetGroup.DBSubnetGroupDescription. Emulator: those are omitted and VpcSecurityGroups is the wrong type. Why: a client that takes the subnet/AZ placement or the CA identifier straight from the create response gets nothing. Fix: as cause 1, in the CreateDBInstance response path. Traces: rds-delete-groups-in-use-by-instance#task#8.

Cause 4 of 5 — DeleteDBInstance response envelope. AWS returns the same complete DBInstance in DeleteDBInstanceResult. Emulator: the same ~30 members are missing (VPCNetworkingEnabled, DBParameterGroups, DatabaseInsightsMode, PendingModifiedValues, DBSubnetGroup.VpcId/Subnets/DBSubnetGroupDescription, TagList, ...). Fix: as cause 1, in the DeleteDBInstance response path. Traces: rds-create-db-instance-catalog-triple#task#15.

Cause 5 of 5 — DescribeDBInstances after a create: DBSubnetGroup.VpcId is missing, and EngineVersion is echoed back as the client-supplied nonexistent '8.0.99' where AWS reports the resolved '8.0.39'. AWS: DBInstance.EngineVersion is the version RDS actually provisioned, and DBSubnetGroup.VpcId identifies the VPC of the subnet group. Emulator: stores and replays the request value and omits VpcId. Why: version-dependent client logic reads a version that does not exist. Fix: the CreateDBInstance state write (resolve EngineVersion through the engine catalog) and the DBSubnetGroup projection. Traces: rds-create-db-instance-catalog-triple#task#14.

## Evidence
Cause 1: rds-db-instance-core-lifecycle#task#2 — 200/200, ~29 DBInstance members reported 'missing', e.g. $...DBInstance.DBParameterGroups missing (expected {'DBParameterGroup': {'DBParameterGroupName': 'default.mysql8.4', 'ParameterApplyStatus': 'in-sync'}}, got None); $...DBInstance.VpcSecurityGroups type_mismatch (expected a VpcSecurityGroupMembership, got '').
Cause 2: rds-describe-instance-case-insensitive#task#2 — same set plus Endpoint.HostedZoneId, CACertificateIdentifier, DBSubnetGroup missing.
Cause 3: rds-delete-groups-in-use-by-instance#task#8 — CreateDBInstanceResult.DBInstance.DBSubnetGroup.Subnets / VpcId / DBSubnetGroupDescription missing, CACertificateIdentifier missing (expected 'rds-ca-rsa2048-g1').
Cause 4: rds-create-db-instance-catalog-triple#task#15 — DeleteDBInstanceResult.DBInstance missing ~32 members.
Cause 5: rds-create-db-instance-catalog-triple#task#14 — EngineVersion expected '8.0.39', got '8.0.99'; DBSubnetGroup.VpcId missing.

## Steps to Reproduce
1) Replay the wire corpus (replay_wire_corpus).
2) Fixture rds-db-instance-core-lifecycle step task#2: create an instance then DescribeDBInstances and diff the DBInstance element against the recorded response.
3) Repeat for rds-describe-instance-case-insensitive#task#2, rds-delete-groups-in-use-by-instance#task#8 (create envelope) and rds-create-db-instance-catalog-triple#task#15 (delete envelope).
4) Fixture rds-create-db-instance-catalog-triple#task#14: DescribeDBInstances and compare DBInstance.EngineVersion with the engine version AWS resolved.
