Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-cluster-lifecycle
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-cluster-lifecycle
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: docdb|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.CustomDataGroupPresent
Issue-Key: docdb|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.DocdbAnalyticsEnabled
Issue-Key: docdb|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.HostedZoneId
Issue-Key: docdb|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.InternetAccessGatewayEnabled
Issue-Key: docdb|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.VPCNetworkingEnabled
Issue-Key: docdb|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.VpcSecurityGroups
Issue-Key: docdb|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.CustomDataGroupPresent
Issue-Key: docdb|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.DBActivityStreamStatus
Issue-Key: docdb|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.DocdbAnalyticsEnabled
Issue-Key: docdb|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.EarliestRestorableTime
Issue-Key: docdb|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.HostedZoneId
Issue-Key: docdb|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.InternetAccessGatewayEnabled
Issue-Key: docdb|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.LatestRestorableTime
Issue-Key: docdb|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.VPCNetworkingEnabled
Issue-Key: docdb|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.VpcSecurityGroups
Issue-Commit: b357a994596866b4c3c5b5df5566beb9dd5436ae

# DocDB DBCluster projection omits VPC security groups and cluster timestamps

> _Note: this issue was identified against the code base at commit `b357a994596866b4c3c5b5df5566beb9dd5436ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
This issue bundles TWO causes with one root fault: the docdb DBCluster state record and its awsQuery projection drop the cluster's networking membership, its Route 53 hosted zone, its point-in-time-restore window, its activity-stream status and the DocDB-specific networking flags. The write path and the read path are separately wrong, so a fixer must close BOTH causes below before submitting.

Cause 1 of 2: CreateDBCluster's returned DBCluster drops VpcSecurityGroups and HostedZoneId and the DocDB networking flags.
AWS: Smithy (com.amazonaws.docdb#AmazonRDSv19) CreateDBCluster takes VpcSecurityGroupIds (VpcSecurityGroupIdList) and returns CreateDBClusterResult{DBCluster}. The DBCluster shape models VpcSecurityGroups (VpcSecurityGroupMembershipList, "Provides a list of virtual private cloud (VPC) security groups that the cluster belongs to") and HostedZoneId ("Specifies the ID that Amazon Route 53 assigns when you create a hosted zone"). AWS therefore echoes one VpcSecurityGroupMembership per requested security group, each with VpcSecurityGroupId = the id from the request and Status 'active', and a non-empty HostedZoneId generated for the cluster. The recording also shows the DocDB wire members VPCNetworkingEnabled ('true' for a VPC cluster), InternetAccessGatewayEnabled ('false'), DocdbAnalyticsEnabled ('false') and CustomDataGroupPresent ('false') on every DBCluster envelope (recorded behavior only - these four are not in the on-disk docdb DBCluster shape).
Emulator: the 200 CreateDBClusterResult.DBCluster serializes VpcSecurityGroups as an EMPTY element (type_mismatch against the expected VpcSecurityGroupMembership) even though the request named a security group, and has no HostedZoneId, no VPCNetworkingEnabled, no InternetAccessGatewayEnabled, no DocdbAnalyticsEnabled and no CustomDataGroupPresent elements.
Why: a client that reads back its own VpcSecurityGroupIds to confirm the cluster's networking sees none attached, and a client that builds a Route 53 alias or a connection record from HostedZoneId has nothing to read; the requested security-group association is simply lost.
Fix: the docdb CreateDBCluster handler must persist the requested VpcSecurityGroupIds (with membership status) and a generated hosted-zone id in the cluster state record, and the DBCluster serializer must project VpcSecurityGroups as a list of VpcSecurityGroupMembership plus HostedZoneId and the four DocDB networking flags.

Cause 2 of 2: DescribeDBClusters drops the same members and additionally DBActivityStreamStatus, EarliestRestorableTime and LatestRestorableTime.
AWS: DescribeDBClusters returns the identical DBCluster shape, which models EarliestRestorableTime ("The earliest time to which a database can be restored with point-in-time restore") and LatestRestorableTime ("Specifies the latest time to which a database can be restored with point-in-time restore") as TStamp members; AWS emits both for a cluster with backups enabled, and the recording shows DBActivityStreamStatus 'stopped' for a cluster that never started an activity stream (recorded behavior only for that member's presence on docdb). The timestamp VALUES are per-run clock values and are not requirements - their PRESENCE and ISO-8601 shape are.
Emulator: the 200 DescribeDBClustersResult.DBClusters.DBCluster has no EarliestRestorableTime, no LatestRestorableTime, no DBActivityStreamStatus, no HostedZoneId, no VPCNetworkingEnabled / InternetAccessGatewayEnabled / DocdbAnalyticsEnabled / CustomDataGroupPresent, and again an empty VpcSecurityGroups element.
Why: a client that gates a point-in-time restore on LatestRestorableTime, or that polls DBActivityStreamStatus before starting/stopping a stream, can never make progress against the emulator.
Fix: the cluster state record must carry the restore window and the activity-stream status, and the shared DBCluster projection used by the describe handler must emit them alongside the Cause-1 members.

Traces: docdb-cluster-lifecycle#task#7 (Cause 1), docdb-cluster-lifecycle#task#8 (Cause 2).

## Evidence
Cause 1 - fixture docdb-cluster-lifecycle step task#7, CreateDBCluster, expected 200 / actual 200: '$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.VpcSecurityGroups: type_mismatch (expected {VpcSecurityGroupMembership: {VpcSecurityGroupId: <the sg id from the request>, Status: active}}, got "")'; '....DBCluster.HostedZoneId: missing'; '....DBCluster.VPCNetworkingEnabled: missing (expected true)'; '....DBCluster.InternetAccessGatewayEnabled: missing (expected false)'; '....DBCluster.DocdbAnalyticsEnabled: missing (expected false)'; '....DBCluster.CustomDataGroupPresent: missing (expected false)'. The recorded sg id is a per-run value; the requirement is that the id supplied in VpcSecurityGroupIds comes back with Status 'active'.
Cause 2 - fixture docdb-cluster-lifecycle step task#8, DescribeDBClusters, 200/200: the same six keys missing/empty under $.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster, plus '....DBActivityStreamStatus: missing (expected stopped)', '....EarliestRestorableTime: missing' and '....LatestRestorableTime: missing' (the recorded instants are per-run clock values).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Cause 1: fixture docdb-cluster-lifecycle, step task#7 - Action=CreateDBCluster with Engine=docdb, a DBSubnetGroupName, and VpcSecurityGroupIds.member.1=<an existing sg id>. Inspect the 200 CreateDBClusterResult.DBCluster: VpcSecurityGroups is an empty element instead of one VpcSecurityGroupMembership{VpcSecurityGroupId=<that id>, Status=active}, and HostedZoneId / VPCNetworkingEnabled / InternetAccessGatewayEnabled / DocdbAnalyticsEnabled / CustomDataGroupPresent are absent.
3. Cause 2: same fixture, step task#8 - Action=DescribeDBClusters with that DBClusterIdentifier. The described DBCluster shows the same omissions and additionally lacks DBActivityStreamStatus ('stopped'), EarliestRestorableTime and LatestRestorableTime.
