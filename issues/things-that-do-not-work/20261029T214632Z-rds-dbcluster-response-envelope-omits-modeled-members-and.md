Issue-Source: trace-sweep-divergence
Issue-Trace: rds-aurora-serverless-v2-config-read-back
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-aurora-serverless-v2-config-read-back rds-db-cluster-lifecycle rds-db-cluster-lifecycle-with-snapshot rds-multi-cluster-isolation rds-db-cluster-state-roles-and-capacity-operations
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.CustomDataGroupPresent
Issue-Key: rds|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.DBActivityStreamStatus
Issue-Key: rds|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.DBClusterParameterGroup
Issue-Key: rds|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.DBSubnetGroup
Issue-Key: rds|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.DocdbAnalyticsEnabled
Issue-Key: rds|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.EarliestRestorableTime
Issue-Key: rds|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.EngineVersion
Issue-Key: rds|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.HostedZoneId
Issue-Key: rds|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.InternetAccessGatewayEnabled
Issue-Key: rds|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.KmsKeyId
Issue-Key: rds|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.LatestRestorableTime
Issue-Key: rds|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.ServerlessV2PlatformVersion
Issue-Key: rds|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.ServerlessV2ScalingConfiguration.MaxCapacity
Issue-Key: rds|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.VPCNetworkingEnabled
Issue-Key: rds|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.VpcSecurityGroups
Issue-Key: rds|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.BackupRetentionPeriod
Issue-Key: rds|DeleteDBCluster|2xx->2xx|$.DeleteDBClusterResponse.DeleteDBClusterResult.DBCluster.BackupRetentionPeriod
Issue-Key: rds|DeleteDBCluster|2xx->2xx|$.DeleteDBClusterResponse.DeleteDBClusterResult.DBCluster.CustomDataGroupPresent
Issue-Key: rds|DeleteDBCluster|2xx->2xx|$.DeleteDBClusterResponse.DeleteDBClusterResult.DBCluster.DBClusterParameterGroup
Issue-Key: rds|DeleteDBCluster|2xx->2xx|$.DeleteDBClusterResponse.DeleteDBClusterResult.DBCluster.DBSubnetGroup
Issue-Key: rds|DeleteDBCluster|2xx->2xx|$.DeleteDBClusterResponse.DeleteDBClusterResult.DBCluster.DocdbAnalyticsEnabled
Issue-Key: rds|DeleteDBCluster|2xx->2xx|$.DeleteDBClusterResponse.DeleteDBClusterResult.DBCluster.EarliestRestorableTime
Issue-Key: rds|DeleteDBCluster|2xx->2xx|$.DeleteDBClusterResponse.DeleteDBClusterResult.DBCluster.EngineVersion
Issue-Key: rds|DeleteDBCluster|2xx->2xx|$.DeleteDBClusterResponse.DeleteDBClusterResult.DBCluster.HostedZoneId
Issue-Key: rds|DeleteDBCluster|2xx->2xx|$.DeleteDBClusterResponse.DeleteDBClusterResult.DBCluster.InternetAccessGatewayEnabled
Issue-Key: rds|DeleteDBCluster|2xx->2xx|$.DeleteDBClusterResponse.DeleteDBClusterResult.DBCluster.LatestRestorableTime
Issue-Key: rds|DeleteDBCluster|2xx->2xx|$.DeleteDBClusterResponse.DeleteDBClusterResult.DBCluster.Status
Issue-Key: rds|DeleteDBCluster|2xx->2xx|$.DeleteDBClusterResponse.DeleteDBClusterResult.DBCluster.VPCNetworkingEnabled
Issue-Key: rds|DeleteDBCluster|2xx->2xx|$.DeleteDBClusterResponse.DeleteDBClusterResult.DBCluster.VpcSecurityGroups
Issue-Key: rds|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.CustomDataGroupPresent
Issue-Key: rds|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.DBSubnetGroup
Issue-Key: rds|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.DocdbAnalyticsEnabled
Issue-Key: rds|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.EngineVersion
Issue-Key: rds|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.HostedZoneId
Issue-Key: rds|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.InternetAccessGatewayEnabled
Issue-Key: rds|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.VPCNetworkingEnabled
Issue-Key: rds|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.VpcSecurityGroups
Issue-Key: rds|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.CustomDataGroupPresent
Issue-Key: rds|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.DBSubnetGroup
Issue-Key: rds|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.DocdbAnalyticsEnabled
Issue-Key: rds|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.EarliestRestorableTime
Issue-Key: rds|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.EngineVersion
Issue-Key: rds|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.HostedZoneId
Issue-Key: rds|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.InternetAccessGatewayEnabled
Issue-Key: rds|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.LatestRestorableTime
Issue-Key: rds|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.Status
Issue-Key: rds|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.VPCNetworkingEnabled
Issue-Key: rds|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.VpcSecurityGroups
Issue-Key: rds|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.DBClusterParameterGroup
Issue-Commit: 9919ffd222ff0dfbfa1095fff748926e9311733c

# RDS DBCluster response envelope omits modeled members and reports stale engine defaults

> _Note: this issue was identified against the code base at commit `9919ffd222ff0dfbfa1095fff748926e9311733c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBClusters

## Description
Every operation that serializes a DBCluster emits an envelope missing the same group of modeled members, and fills several others with stale or transient values. The gap is in the shared DBCluster serializer plus the engine-default and cluster-networking state, so it reproduces identically on Create, Modify, Delete and Describe. 8 causes.

Shared AWS behavior (all eight causes): the recorded DBCluster envelope always carries VPCNetworkingEnabled, DBSubnetGroup, VpcSecurityGroups (a VpcSecurityGroupMembership with VpcSecurityGroupId and Status 'active'), InternetAccessGatewayEnabled, DocdbAnalyticsEnabled, CustomDataGroupPresent and HostedZoneId, and for an available cluster also EarliestRestorableTime, LatestRestorableTime, DBActivityStreamStatus. Recorded behavior only for the member values; the members themselves are modeled on the DBCluster shape.
Emulator (all eight): those members are absent (None) or serialized as an empty string instead of the membership structure, and the Aurora default DBClusterParameterGroup/EngineVersion are a version behind the recorded catalog.
Why (all eight): a client reading a cluster cannot find its subnet group, security groups, hosted zone or restore window, so DNS/VPC wiring and point-in-time-restore logic have nothing to read; a client that pins the engine default gets the wrong major/minor version.
Fix (all eight): the shared DBCluster serializer, plus the Aurora engine-default table and the cluster's VPC/subnet-group state.

Cause 1 of 8: DescribeDBClusters for an Aurora Serverless v2 cluster — additionally ServerlessV2ScalingConfiguration.MaxCapacity is serialized as '1' where the recording has the decimal form '1.0', ServerlessV2PlatformVersion and KmsKeyId are absent, and DBSubnetGroup/DBClusterParameterGroup/EngineVersion diverge. Traces: rds-aurora-serverless-v2-config-read-back#task#2.
Cause 2 of 8: DescribeDBClusters for a provisioned Aurora PostgreSQL cluster — additionally BackupRetentionPeriod is reported as 7 where the request/recording give 1. Traces: rds-db-cluster-lifecycle#task#5.
Cause 3 of 8: DeleteDBCluster — additionally Status is 'deleting' where the recording has 'available' (AWS returns the pre-delete cluster snapshot of state in the Delete response). Traces: rds-db-cluster-lifecycle#teardown#6.
Cause 4 of 8: CreateDBCluster (aurora-mysql) — additionally EngineVersion is the stale default 8.0.mysql_aurora.3.08.0 instead of the recorded 8.0.mysql_aurora.3.10.3. Traces: rds-db-cluster-lifecycle-with-snapshot#task#1; also rds-multi-cluster-isolation#setup#2.
Cause 5 of 8: DescribeDBClusters (aurora-mysql) — same omissions and stale EngineVersion. Traces: rds-db-cluster-lifecycle-with-snapshot#task#2; also rds-multi-cluster-isolation#task#4.
Cause 6 of 8: ModifyDBCluster — additionally Status is 'modifying' where the recording has 'available'. Traces: rds-db-cluster-lifecycle-with-snapshot#task#3.
Cause 7 of 8: DeleteDBCluster on a cluster with a final snapshot — additionally Status is 'deleting' where the recording has 'backing-up'. Traces: rds-db-cluster-lifecycle-with-snapshot#teardown#10; also rds-multi-cluster-isolation#teardown#6.
Cause 8 of 8: CreateDBCluster (aurora-postgresql) — additionally the default DBClusterParameterGroup is default.aurora-postgresql16 where the recording has default.aurora-postgresql17. Traces: rds-db-cluster-state-roles-and-capacity-operations#setup#1; also rds-multi-cluster-isolation#setup#1.

## Evidence
Replay cluster details (status 200 matched in all eight; the body diverged):
1. rds-aurora-serverless-v2-config-read-back#task#2 DescribeDBClusters: DBCluster.VPCNetworkingEnabled missing (expected 'true'); DBActivityStreamStatus missing (expected 'stopped'); EarliestRestorableTime missing; DBSubnetGroup missing (expected 'default'); VpcSecurityGroups type_mismatch (expected {'VpcSecurityGroupMembership': {...'Status': 'active'}}, got ''); DBClusterParameterGroup expected 'default.aurora-postgresql17' got 'default.aurora-postgresql16'; ServerlessV2PlatformVersion missing (expected '4'); InternetAccessGatewayEnabled/DocdbAnalyticsEnabled/CustomDataGroupPresent missing (expected 'false'); EngineVersion expected '17.9' got '16.6'; KmsKeyId missing; ServerlessV2ScalingConfiguration.MaxCapacity expected '1.0' got '1'.
2. rds-db-cluster-lifecycle#task#5 DescribeDBClusters: the same missing members plus BackupRetentionPeriod expected '1' got '7', LatestRestorableTime missing, HostedZoneId missing (expected 'Z1PVIF0B656C1W').
3. rds-db-cluster-lifecycle#teardown#6 DeleteDBCluster: same missing members plus Status expected 'available' got 'deleting'.
4. rds-db-cluster-lifecycle-with-snapshot#task#1 CreateDBCluster: same missing members plus EngineVersion expected '8.0.mysql_aurora.3.10.3' got '8.0.mysql_aurora.3.08.0'.
5. rds-db-cluster-lifecycle-with-snapshot#task#2 DescribeDBClusters: same.
6. rds-db-cluster-lifecycle-with-snapshot#task#3 ModifyDBCluster: same missing members plus Status expected 'available' got 'modifying'.
7. rds-db-cluster-lifecycle-with-snapshot#teardown#10 DeleteDBCluster: same missing members plus Status expected 'backing-up' got 'deleting'.
8. rds-db-cluster-state-roles-and-capacity-operations#setup#1 CreateDBCluster: same missing members plus DBClusterParameterGroup expected 'default.aurora-postgresql17' got 'default.aurora-postgresql16' and EngineVersion '16.6'.
(Hosted-zone id, vpc/sg ids and version strings above are the recorded catalog's values; the requirement is that the members be present and carry the engine default the catalog publishes, not these literal ids.)

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Re-run rds-aurora-serverless-v2-config-read-back#task#2 and every step in trace_ids; each creates/modifies/deletes/reads an Aurora cluster and the replay diffs the DBCluster envelope against the recording.
