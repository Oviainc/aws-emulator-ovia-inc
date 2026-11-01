Issue-Source: trace-sweep-divergence
Issue-Trace: neptune-db-cluster-lifecycle-and-custom-endpoint
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: neptune-db-cluster-lifecycle-and-custom-endpoint neptune-db-cluster-snapshot-lifecycle neptune-db-subnet-group-lifecycle neptune-cluster-parameter-group-lifecycle neptune-parameter-group-family-not-engine-scoped neptune-db-parameter-group-copy-and-lifecycle
Issue-Shape-Id: com.amazonaws.neptune#AmazonRDSv19
Issue-Key: neptune|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.AutoMinorVersionUpgrade
Issue-Key: neptune|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.CustomDataGroupPresent
Issue-Key: neptune|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.DBClusterParameterGroup
Issue-Key: neptune|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.DocdbAnalyticsEnabled
Issue-Key: neptune|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.HostedZoneId
Issue-Key: neptune|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.InternetAccessGatewayEnabled
Issue-Key: neptune|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.MasterUsername
Issue-Key: neptune|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.VPCNetworkingEnabled
Issue-Key: neptune|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.VpcSecurityGroups
Issue-Key: neptune|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.AutoMinorVersionUpgrade
Issue-Key: neptune|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.CustomDataGroupPresent
Issue-Key: neptune|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.DBActivityStreamStatus
Issue-Key: neptune|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.DBClusterParameterGroup
Issue-Key: neptune|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.DocdbAnalyticsEnabled
Issue-Key: neptune|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.EarliestRestorableTime
Issue-Key: neptune|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.HostedZoneId
Issue-Key: neptune|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.InternetAccessGatewayEnabled
Issue-Key: neptune|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.LatestRestorableTime
Issue-Key: neptune|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.MasterUsername
Issue-Key: neptune|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.VPCNetworkingEnabled
Issue-Key: neptune|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.VpcSecurityGroups
Issue-Key: neptune|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.AutoMinorVersionUpgrade
Issue-Key: neptune|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.CustomDataGroupPresent
Issue-Key: neptune|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.CustomEndpoints
Issue-Key: neptune|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.DBClusterParameterGroup
Issue-Key: neptune|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.DocdbAnalyticsEnabled
Issue-Key: neptune|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.EarliestRestorableTime
Issue-Key: neptune|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.HostedZoneId
Issue-Key: neptune|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.InternetAccessGatewayEnabled
Issue-Key: neptune|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.LatestRestorableTime
Issue-Key: neptune|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.MasterUsername
Issue-Key: neptune|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.Status
Issue-Key: neptune|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.VPCNetworkingEnabled
Issue-Key: neptune|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.VpcSecurityGroups
Issue-Key: neptune|CreateDBClusterSnapshot|2xx->2xx|$.CreateDBClusterSnapshotResponse.CreateDBClusterSnapshotResult.DBClusterSnapshot.AllocatedStorage
Issue-Key: neptune|CreateDBClusterSnapshot|2xx->2xx|$.CreateDBClusterSnapshotResponse.CreateDBClusterSnapshotResult.DBClusterSnapshot.BackupRetentionPeriod
Issue-Key: neptune|CreateDBClusterSnapshot|2xx->2xx|$.CreateDBClusterSnapshotResponse.CreateDBClusterSnapshotResult.DBClusterSnapshot.CustomDataGroupPresent
Issue-Key: neptune|CreateDBClusterSnapshot|2xx->2xx|$.CreateDBClusterSnapshotResponse.CreateDBClusterSnapshotResult.DBClusterSnapshot.IAMDatabaseAuthenticationEnabled
Issue-Key: neptune|CreateDBClusterSnapshot|2xx->2xx|$.CreateDBClusterSnapshotResponse.CreateDBClusterSnapshotResult.DBClusterSnapshot.LicenseModel
Issue-Key: neptune|CreateDBClusterSnapshot|2xx->2xx|$.CreateDBClusterSnapshotResponse.CreateDBClusterSnapshotResult.DBClusterSnapshot.MasterUsername
Issue-Key: neptune|CreateDBClusterSnapshot|2xx->2xx|$.CreateDBClusterSnapshotResponse.CreateDBClusterSnapshotResult.DBClusterSnapshot.Port
Issue-Key: neptune|CreateDBClusterSnapshot|2xx->2xx|$.CreateDBClusterSnapshotResponse.CreateDBClusterSnapshotResult.DBClusterSnapshot.PreferredBackupWindow
Issue-Key: neptune|CreateDBClusterSnapshot|2xx->2xx|$.CreateDBClusterSnapshotResponse.CreateDBClusterSnapshotResult.DBClusterSnapshot.VpcId
Issue-Key: neptune|DescribeDBClusterSnapshots|2xx->2xx|$.DescribeDBClusterSnapshotsResponse.DescribeDBClusterSnapshotsResult.DBClusterSnapshots.DBClusterSnapshot.AllocatedStorage
Issue-Key: neptune|DescribeDBClusterSnapshots|2xx->2xx|$.DescribeDBClusterSnapshotsResponse.DescribeDBClusterSnapshotsResult.DBClusterSnapshots.DBClusterSnapshot.BackupRetentionPeriod
Issue-Key: neptune|DescribeDBClusterSnapshots|2xx->2xx|$.DescribeDBClusterSnapshotsResponse.DescribeDBClusterSnapshotsResult.DBClusterSnapshots.DBClusterSnapshot.FullSnapshotSizeInBytes
Issue-Key: neptune|DescribeDBClusterSnapshots|2xx->2xx|$.DescribeDBClusterSnapshotsResponse.DescribeDBClusterSnapshotsResult.DBClusterSnapshots.DBClusterSnapshot.IAMDatabaseAuthenticationEnabled
Issue-Key: neptune|DescribeDBClusterSnapshots|2xx->2xx|$.DescribeDBClusterSnapshotsResponse.DescribeDBClusterSnapshotsResult.DBClusterSnapshots.DBClusterSnapshot.LicenseModel
Issue-Key: neptune|DescribeDBClusterSnapshots|2xx->2xx|$.DescribeDBClusterSnapshotsResponse.DescribeDBClusterSnapshotsResult.DBClusterSnapshots.DBClusterSnapshot.MasterUsername
Issue-Key: neptune|DescribeDBClusterSnapshots|2xx->2xx|$.DescribeDBClusterSnapshotsResponse.DescribeDBClusterSnapshotsResult.DBClusterSnapshots.DBClusterSnapshot.Port
Issue-Key: neptune|DescribeDBClusterSnapshots|2xx->2xx|$.DescribeDBClusterSnapshotsResponse.DescribeDBClusterSnapshotsResult.DBClusterSnapshots.DBClusterSnapshot.PreferredBackupWindow
Issue-Key: neptune|DescribeDBClusterSnapshots|2xx->2xx|$.DescribeDBClusterSnapshotsResponse.DescribeDBClusterSnapshotsResult.DBClusterSnapshots.DBClusterSnapshot.VpcId
Issue-Key: neptune|CreateDBSubnetGroup|2xx->2xx|$.CreateDBSubnetGroupResponse.CreateDBSubnetGroupResult.DBSubnetGroup.Subnets.Subnet[*]
Issue-Key: neptune|CreateDBSubnetGroup|2xx->2xx|$.CreateDBSubnetGroupResponse.CreateDBSubnetGroupResult.DBSubnetGroup.SupportedNetworkTypes
Issue-Key: neptune|CreateDBSubnetGroup|2xx->2xx|$.CreateDBSubnetGroupResponse.CreateDBSubnetGroupResult.DBSubnetGroup.VpcId
Issue-Key: neptune|DescribeDBSubnetGroups|2xx->2xx|$.DescribeDBSubnetGroupsResponse.DescribeDBSubnetGroupsResult.DBSubnetGroups.DBSubnetGroup.Subnets.Subnet[*]
Issue-Key: neptune|DescribeDBSubnetGroups|2xx->2xx|$.DescribeDBSubnetGroupsResponse.DescribeDBSubnetGroupsResult.DBSubnetGroups.DBSubnetGroup.SupportedNetworkTypes
Issue-Key: neptune|DescribeDBSubnetGroups|2xx->2xx|$.DescribeDBSubnetGroupsResponse.DescribeDBSubnetGroupsResult.DBSubnetGroups.DBSubnetGroup.VpcId
Issue-Key: neptune|ModifyDBSubnetGroup|2xx->2xx|$.ModifyDBSubnetGroupResponse.ModifyDBSubnetGroupResult.DBSubnetGroup.Subnets.Subnet[*]
Issue-Key: neptune|ModifyDBSubnetGroup|2xx->2xx|$.ModifyDBSubnetGroupResponse.ModifyDBSubnetGroupResult.DBSubnetGroup.SupportedNetworkTypes
Issue-Key: neptune|ModifyDBSubnetGroup|2xx->2xx|$.ModifyDBSubnetGroupResponse.ModifyDBSubnetGroupResult.DBSubnetGroup.VpcId
Issue-Key: neptune|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter.AllowedValues
Issue-Key: neptune|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter.ApplyType
Issue-Key: neptune|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter.DataType
Issue-Key: neptune|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter.Description
Issue-Key: neptune|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter.IsModifiable
Issue-Key: neptune|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter.SupportedEngineModes
Issue-Key: neptune|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult.Parameters.Parameter.ApplyType
Issue-Key: neptune|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult.Parameters.Parameter.Description
Issue-Commit: c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c

# Neptune resource-describing responses omit modelled members of the resource they return

> _Note: this issue was identified against the code base at commit `c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBClusters

## Description
The Neptune emulator's serializers project only a subset of each returned resource structure: the cluster, cluster-snapshot, subnet-group and parameter projections each drop modelled members, and the ones that are emitted sometimes carry a value AWS does not report for a freshly created resource. Ten independent causes, all in the same projection layer. THIS ISSUE HAS 10 CAUSES — all ten must be closed before submitting.

Cause 1 of 10: CreateDBCluster returns an incomplete DBCluster. AWS: Smithy com.amazonaws.neptune#DBCluster is returned in CreateDBClusterResult; the recorded response carries MasterUsername (the request's MasterUsername), HostedZoneId, VpcSecurityGroups as a VpcSecurityGroupMembership list (VpcSecurityGroupId + Status 'active') for the cluster's security groups, and the engine-default DBClusterParameterGroup for the engine version actually created; AutoMinorVersionUpgrade reflects the request. Emulator: those members are absent (MasterUsername, HostedZoneId, VPCNetworkingEnabled, InternetAccessGatewayEnabled, DocdbAnalyticsEnabled, CustomDataGroupPresent all None), VpcSecurityGroups serializes as an empty scalar instead of a membership list, DBClusterParameterGroup names the default of a different engine-version family than the one created, and AutoMinorVersionUpgrade is hardcoded true instead of honouring the request. Why: a client that reads MasterUsername, resolves the writer endpoint through HostedZoneId, or inspects the attached security groups gets nothing and cannot proceed. Fix: the CreateDBCluster handler's DBCluster projection and the cluster record it builds in state. Traces: neptune-db-cluster-lifecycle-and-custom-endpoint#task#1.

Cause 2 of 10: DescribeDBClusters repeats the same gap on read. AWS: DescribeDBClusters returns the same DBCluster shape and additionally reports EarliestRestorableTime, LatestRestorableTime and DBActivityStreamStatus ('stopped' when no activity stream was ever started). Emulator: all of those are missing, plus the same MasterUsername/HostedZoneId/VpcSecurityGroups/DBClusterParameterGroup/AutoMinorVersionUpgrade divergences as cause 1. Why: the read path is where clients poll for cluster state, so the gap is permanent, not just a create-time artifact. Fix: the DescribeDBClusters handler's DBCluster projection (shared with CreateDBCluster). Traces: neptune-db-cluster-lifecycle-and-custom-endpoint#task#2.

Cause 3 of 10: ModifyDBCluster leaves the cluster in 'modifying' and drops CustomEndpoints. AWS: the recorded ModifyDBCluster response reports Status 'available' for a modification that needs no downtime, and lists the cluster's custom endpoints in CustomEndpoints as a member list of endpoint addresses. Emulator: Status is 'available'->'modifying', CustomEndpoints serializes as an empty scalar even though a custom endpoint exists on the cluster, and the same missing members as cause 1/2 recur. Why: a client that waits for Status 'available' after a modify never observes it, and custom endpoints created on the cluster are invisible. Fix: the ModifyDBCluster handler's status transition and its DBCluster projection. Traces: neptune-db-cluster-lifecycle-and-custom-endpoint#task#9.

Cause 4 of 10: CreateDBClusterSnapshot returns an incomplete DBClusterSnapshot and the wrong Port. AWS: Smithy com.amazonaws.neptune#DBClusterSnapshot models AllocatedStorage, Port, VpcId, MasterUsername, LicenseModel and IAMDatabaseAuthenticationEnabled; the recording shows AllocatedStorage, PreferredBackupWindow, BackupRetentionPeriod, VpcId (the source cluster's VPC), MasterUsername (the source cluster's master user), LicenseModel 'neptune', IAMDatabaseAuthenticationEnabled, and Port 0 while the snapshot is still being created (the port is only reported once known). Emulator: all of those members are absent and Port is emitted as the cluster's listening port (8182) instead of 0. Why: a client sizing a restore from AllocatedStorage/VpcId, or checking the snapshot's auth setting, reads nulls. Fix: the CreateDBClusterSnapshot handler's DBClusterSnapshot projection, which must copy the source cluster's attributes into the snapshot record in state. Traces: neptune-db-cluster-snapshot-lifecycle#task#1.

Cause 5 of 10: DescribeDBClusterSnapshots repeats cause 4 on read and also omits FullSnapshotSizeInBytes. AWS: the same DBClusterSnapshot projection, with AllocatedStorage and Port reported as 0 for a snapshot of an empty cluster and FullSnapshotSizeInBytes present. Emulator: AllocatedStorage, PreferredBackupWindow, BackupRetentionPeriod, VpcId, MasterUsername, LicenseModel, IAMDatabaseAuthenticationEnabled and FullSnapshotSizeInBytes are all missing and Port is 8182 instead of 0. Why: the snapshot listing is the only way to discover a snapshot's properties before restoring it. Fix: the DescribeDBClusterSnapshots handler's DBClusterSnapshot projection. Traces: neptune-db-cluster-snapshot-lifecycle#task#3.

Cause 6 of 10: CreateDBSubnetGroup drops VpcId, SupportedNetworkTypes and each subnet's availability zone. AWS: Smithy com.amazonaws.neptune#DBSubnetGroup models VpcId ('Provides the VpcId of the DB subnet group') and SupportedNetworkTypes ('Valid network types include IPV4 and DUAL'), and com.amazonaws.neptune#Subnet models SubnetAvailabilityZone; the recording reports VpcId, SupportedNetworkTypes containing IPV4, and per subnet SubnetAvailabilityZone.Name plus an (empty) SubnetOutpost. Emulator: VpcId and SupportedNetworkTypes are absent and every Subnet element carries only SubnetIdentifier and SubnetStatus. Why: a client that places a cluster by resolving the subnet group's VPC and availability zones cannot do so; AZ coverage is unknowable. Fix: the CreateDBSubnetGroup handler's DBSubnetGroup/Subnet projection and the subnet-group record in state, which must resolve each subnet's VPC and AZ. Traces: neptune-db-subnet-group-lifecycle#task#1.

Cause 7 of 10: DescribeDBSubnetGroups repeats cause 6 on read — same missing VpcId, SupportedNetworkTypes and per-subnet SubnetAvailabilityZone/SubnetOutpost. Fix: the DescribeDBSubnetGroups handler's DBSubnetGroup projection. Traces: neptune-db-subnet-group-lifecycle#task#2.

Cause 8 of 10: ModifyDBSubnetGroup repeats cause 6 in its response — same missing VpcId, SupportedNetworkTypes and per-subnet SubnetAvailabilityZone/SubnetOutpost. Fix: the ModifyDBSubnetGroup handler's DBSubnetGroup projection. Traces: neptune-db-subnet-group-lifecycle#task#6.

Cause 9 of 10: DescribeDBClusterParameters returns parameters stripped of their metadata. AWS: Smithy com.amazonaws.neptune#Parameter models Description, Source, ApplyType, DataType, AllowedValues and IsModifiable, and DescribeDBClusterParameters reports them for every parameter (plus SupportedEngineModes in the recorded wire shape) — e.g. for the audit-log toggle, DataType 'boolean', AllowedValues '0,1', ApplyType 'static', IsModifiable 'true' and the engine's own description text. Emulator: AllowedValues, ApplyType, DataType, Description, IsModifiable and SupportedEngineModes are all None; only name/value survive. Why: a console or client that renders editable parameters with their allowed ranges has no type, range or modifiability information and cannot validate a change before sending it. Fix: the DescribeDBClusterParameters handler's Parameter projection and the engine parameter catalog it reads from in state. Traces: neptune-cluster-parameter-group-lifecycle#task#6, neptune-parameter-group-family-not-engine-scoped#task#4.

Cause 10 of 10: DescribeDBParameters reports the wrong ApplyType and a non-Neptune description for a Neptune parameter. AWS: the recorded DescribeDBParameters response gives the graph-query-timeout parameter ApplyType 'static' and the Neptune engine's own short description for it. Emulator: ApplyType is 'dynamic' and the Description is a different, longer sentence than the engine publishes. Why: ApplyType decides whether a client must reboot the instance for the change to take effect, so 'dynamic' tells a client no reboot is needed when one is. Fix: the parameter catalog entries behind DescribeDBParameters (same Parameter projection as cause 9). Traces: neptune-db-parameter-group-copy-and-lifecycle#task#4.

## Evidence
Replay of simparity-wire-aws-v1 (13 fixtures, 136 steps): all ten clusters return the expected HTTP status (200) but diverge in the response body.
Cause 1/2/3 (CreateDBCluster/DescribeDBClusters/ModifyDBCluster): expected MasterUsername 'admin', HostedZoneId present, VpcSecurityGroups = {VpcSecurityGroupMembership: {VpcSecurityGroupId, Status 'active'}}, DBClusterParameterGroup 'default.neptune1.4', AutoMinorVersionUpgrade 'false'; got None / '' / 'default.neptune1.3' / 'true'. ModifyDBCluster also expected Status 'available', got 'modifying'; expected CustomEndpoints = {member: <custom endpoint address>}, got ''.
Cause 4/5 (CreateDBClusterSnapshot/DescribeDBClusterSnapshots): expected AllocatedStorage, PreferredBackupWindow, VpcId, MasterUsername, LicenseModel 'neptune', IAMDatabaseAuthenticationEnabled 'false', BackupRetentionPeriod, FullSnapshotSizeInBytes; got None. Expected Port '0', got '8182'.
Cause 6/7/8 (CreateDBSubnetGroup/DescribeDBSubnetGroups/ModifyDBSubnetGroup): expected DBSubnetGroup.VpcId and SupportedNetworkTypes = {member: 'IPV4'}, got None; expected each Subnet = {SubnetIdentifier, SubnetStatus 'Active', SubnetOutpost '', SubnetAvailabilityZone: {Name}}, got {SubnetIdentifier, SubnetStatus} only.
Cause 9 (DescribeDBClusterParameters): expected Parameter.AllowedValues '0,1', ApplyType 'static', DataType 'boolean', IsModifiable 'true', Description present, SupportedEngineModes {member: 'provisioned'}; got None for all six.
Cause 10 (DescribeDBParameters): expected ApplyType 'static', got 'dynamic'; expected the engine's own description text, got a different sentence.

## Steps to Reproduce
1. Replay fixture neptune-db-cluster-lifecycle-and-custom-endpoint and compare steps task#1, task#2 and task#9 bodies against the recording.
2. Replay neptune-db-cluster-snapshot-lifecycle steps task#1 and task#3.
3. Replay neptune-db-subnet-group-lifecycle steps task#1, task#2 and task#6.
4. Replay neptune-cluster-parameter-group-lifecycle step task#6 and neptune-db-parameter-group-copy-and-lifecycle step task#4.
