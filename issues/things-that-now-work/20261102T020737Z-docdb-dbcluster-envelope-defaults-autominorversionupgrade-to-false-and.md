Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-cluster-lifecycle
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-cluster-lifecycle
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: docdb|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.AutoMinorVersionUpgrade
Issue-Key: docdb|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.AutoMinorVersionUpgrade
Issue-Related: 20261029T193448Z-docdb-dbcluster-projection-omits-vpc-security-groups-and.md
Issue-Commit: 73890d4d701534b715f753f6fdea5d402920f69f

# DocDB DBCluster envelope defaults AutoMinorVersionUpgrade to false and still omits the DocDB-only cluster flags

> _Note: this issue was identified against the code base at commit `73890d4d701534b715f753f6fdea5d402920f69f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
This issue bundles TWO causes with one root fault: the shared docdb DBCluster projection (used by both the create path and the describe path) emits AutoMinorVersionUpgrade with the WRONG default and does not emit the DocDB-only cluster flags at all. The write path and the read path are separately wrong, so a fixer must close BOTH causes below before submitting.

Related but NOT covering this: open issue 20261029T193448Z-docdb-dbcluster-projection-omits-vpc-security-groups-and.md ("DocDB DBCluster projection omits VPC security groups and cluster timestamps") names the same two fixture steps, but its replay key set is the VpcSecurityGroups/HostedZoneId/restore-window family; the replay reports these two clusters as UNOWNED because the diverging key set is now AutoMinorVersionUpgrade plus the DocDB-only flags. The AutoMinorVersionUpgrade default is net-new and is not mentioned there at all.

Cause 1 of 2: CreateDBCluster's returned DBCluster reports AutoMinorVersionUpgrade 'false' and omits DocdbAnalyticsEnabled and CustomDataGroupPresent.
AWS: Smithy (com.amazonaws.docdb#AmazonRDSv19) CreateDBCluster returns CreateDBClusterResult{DBCluster: DBCluster}; the on-disk docdb DBCluster shape models NEITHER AutoMinorVersionUpgrade, DocdbAnalyticsEnabled nor CustomDataGroupPresent, and CreateDBClusterMessage has no AutoMinorVersionUpgrade input member — so this is recorded behavior only. The recording shows that AWS's awsQuery DBCluster envelope always carries these three elements, and that for a cluster created WITHOUT any minor-version-upgrade input AWS reports AutoMinorVersionUpgrade 'true' (minor version upgrade is on by default for a DocDB cluster), DocdbAnalyticsEnabled 'false' and CustomDataGroupPresent 'false'.
Emulator: the 200 CreateDBClusterResult.DBCluster serializes AutoMinorVersionUpgrade as 'false', and has no DocdbAnalyticsEnabled and no CustomDataGroupPresent element at all.
Why: a client that reads AutoMinorVersionUpgrade back to decide whether it must explicitly enable automatic minor-version upgrades concludes the cluster is pinned when AWS would have it auto-upgrading, and a client that keys on the presence of DocdbAnalyticsEnabled / CustomDataGroupPresent to detect DocDB-specific capability gets an absent member where AWS always sends a boolean.
Fix: the docdb CreateDBCluster handler's cluster state record must default the minor-version-upgrade flag to enabled rather than disabled, and the shared DBCluster serializer must project AutoMinorVersionUpgrade, DocdbAnalyticsEnabled and CustomDataGroupPresent unconditionally.

Cause 2 of 2: DescribeDBClusters repeats the same three defects and additionally omits DBActivityStreamStatus.
AWS: DescribeDBClusters returns DBClusterMessage{DBClusters: DBClusterList} over the identical DBCluster shape, so the same three elements are expected with the same values ('true'/'false'/'false'). The recording additionally shows DBActivityStreamStatus 'stopped' for a cluster that has never started an activity stream (recorded behavior only — the member is not in the on-disk docdb DBCluster shape).
Emulator: the 200 DescribeDBClustersResult.DBClusters.DBCluster reports AutoMinorVersionUpgrade 'false' and has no DBActivityStreamStatus, no DocdbAnalyticsEnabled and no CustomDataGroupPresent element.
Why: a client that polls DBActivityStreamStatus before calling StartDBClusterActivityStream / StopDBClusterActivityStream can never observe the 'stopped' steady state and loops or faults, and the AutoMinorVersionUpgrade misreport is visible on the read path too, so re-describing the cluster does not correct it.
Fix: the cluster state record must carry an activity-stream status initialised to 'stopped', and the shared DBCluster projection used by the describe handler must emit it alongside the Cause-1 members (the two handlers must share one projection so they cannot drift).

No status or error code diverges in either cause (both are 200 expected / 200 actual); the divergence is entirely in the DBCluster body members.

Traces: docdb-cluster-lifecycle#task#7 (Cause 1), docdb-cluster-lifecycle#task#8 (Cause 2).

## Evidence
Cause 1 — fixture docdb-cluster-lifecycle step task#7, CreateDBCluster, expected 200 / actual 200:
  '$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.AutoMinorVersionUpgrade: value_mismatch (expected "true", got "false")'
  '$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.DocdbAnalyticsEnabled: missing (expected "false", got None)'
  '$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.CustomDataGroupPresent: missing (expected "false", got None)'

Cause 2 — fixture docdb-cluster-lifecycle step task#8, DescribeDBClusters, expected 200 / actual 200:
  '$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.AutoMinorVersionUpgrade: value_mismatch (expected "true", got "false")'
  '$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.DBActivityStreamStatus: missing (expected "stopped", got None)'
  '$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.DocdbAnalyticsEnabled: missing (expected "false", got None)'
  '$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.CustomDataGroupPresent: missing (expected "false", got None)'

All four expected values are fixed defaults for a freshly created DocDB cluster that supplied no minor-version-upgrade, analytics or activity-stream input, not per-run values: the cluster identifier, endpoints, ARNs and timestamps in the same envelope ARE per-run and are not part of this requirement. Model citation: com.amazonaws.docdb#AmazonRDSv19 — CreateDBCluster -> CreateDBClusterResult{DBCluster}, DescribeDBClusters -> DBClusterMessage{DBClusters}; the DBCluster shape models none of these four members and CreateDBClusterMessage has no AutoMinorVersionUpgrade input, so the expectation is recorded behavior only.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Cause 1: fixture docdb-cluster-lifecycle, step task#7 — POST the awsQuery form Action=CreateDBCluster with Engine=docdb, a DBClusterIdentifier and an existing DBSubnetGroupName, and do NOT send any minor-version-upgrade parameter. Inspect the 200 CreateDBClusterResponse/CreateDBClusterResult/DBCluster: AutoMinorVersionUpgrade is 'false' where AWS reports 'true', and there is no DocdbAnalyticsEnabled and no CustomDataGroupPresent element.
3. Cause 2: same fixture, step task#8 — Action=DescribeDBClusters with that DBClusterIdentifier. The described DBCluster shows the same AutoMinorVersionUpgrade 'false' and the same two missing flags, and additionally has no DBActivityStreamStatus element (AWS reports 'stopped' for a cluster with no activity stream).
4. Expected after the fix: both envelopes carry AutoMinorVersionUpgrade=true, DocdbAnalyticsEnabled=false, CustomDataGroupPresent=false, and the describe envelope additionally carries DBActivityStreamStatus=stopped.
