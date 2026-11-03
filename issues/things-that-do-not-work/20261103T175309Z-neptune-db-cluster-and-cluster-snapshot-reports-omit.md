Issue-Source: trace-sweep-divergence
Issue-Trace: neptune-db-cluster-lifecycle-and-custom-endpoint
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: neptune-db-cluster-lifecycle-and-custom-endpoint neptune-db-cluster-snapshot-lifecycle
Issue-Shape-Id: com.amazonaws.neptune#AmazonRDSv19
Issue-Key: neptune|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.AutoMinorVersionUpgrade
Issue-Key: neptune|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.CustomDataGroupPresent
Issue-Key: neptune|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.DocdbAnalyticsEnabled
Issue-Key: neptune|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.HostedZoneId
Issue-Key: neptune|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.AutoMinorVersionUpgrade
Issue-Key: neptune|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.CustomDataGroupPresent
Issue-Key: neptune|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.DBActivityStreamStatus
Issue-Key: neptune|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.DocdbAnalyticsEnabled
Issue-Key: neptune|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.HostedZoneId
Issue-Key: neptune|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.AutoMinorVersionUpgrade
Issue-Key: neptune|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.CustomDataGroupPresent
Issue-Key: neptune|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.CustomEndpoints
Issue-Key: neptune|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.DocdbAnalyticsEnabled
Issue-Key: neptune|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.HostedZoneId
Issue-Key: neptune|CreateDBClusterSnapshot|2xx->2xx|$.CreateDBClusterSnapshotResponse.CreateDBClusterSnapshotResult.DBClusterSnapshot.CustomDataGroupPresent
Issue-Key: neptune|DescribeDBClusterSnapshots|2xx->2xx|$.DescribeDBClusterSnapshotsResponse.DescribeDBClusterSnapshotsResult.DBClusterSnapshots.DBClusterSnapshot.FullSnapshotSizeInBytes
Issue-Commit: c27f8675843924c63f0ee6aedd1be01f08149ed1

# Neptune DB cluster and cluster-snapshot reports omit members and invert defaults the service publishes

> _Note: this issue was identified against the code base at commit `c27f8675843924c63f0ee6aedd1be01f08149ed1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
Every operation that projects a Neptune DBCluster or DBClusterSnapshot renders it from an RDS-generic projection: members the Neptune front door always reports are dropped, a boolean default is inverted, the Route 53 hosted-zone id is synthesised instead of being the engine's published per-region constant, and a custom endpoint created against a cluster is not reflected back in the cluster's own report. Five clusters, all reported with owned_by=[] and stub_path="". THIS ISSUE HAS 5 CAUSES — all five must be closed before submitting.

Cause 1 of 5: CreateDBCluster's DBCluster omits members the service reports and gets AutoMinorVersionUpgrade and HostedZoneId wrong. AWS: the operation returns CreateDBClusterResult.DBCluster (Smithy: com.amazonaws.neptune#DBCluster), whose HostedZoneId member is documented as 'Specifies the ID that Amazon Route 53 assigns when you create a hosted zone' — for this engine that is a FIXED per-region value shared by every cluster in the region, not a per-cluster generated id. Recorded behavior only for the remaining members, which the model does not declare but the wire reports: the created cluster comes back with AutoMinorVersionUpgrade 'false' (the engine does not auto-upgrade minor versions unless asked), DocdbAnalyticsEnabled 'false' and CustomDataGroupPresent 'false'. Emulator: AutoMinorVersionUpgrade is 'true', DocdbAnalyticsEnabled and CustomDataGroupPresent are absent, and HostedZoneId is a per-deployment value rather than the engine's published regional hosted zone. Why: a client reading AutoMinorVersionUpgrade believes the cluster will take minor-version upgrades automatically when it will not, and a client that builds a Route 53 alias record from HostedZoneId builds it against a zone that does not exist; the absent booleans force it to treat 'not reported' as unknown rather than false. Fix: the CreateDBCluster handler's DBCluster projection and the cluster defaults it writes to state. Traces: neptune-db-cluster-lifecycle-and-custom-endpoint#task#1.

Cause 2 of 5: DescribeDBClusters repeats cause 1's projection defects and additionally omits DBActivityStreamStatus. AWS: DescribeDBClusters returns DBClusterMessage.DBClusters, a list of the same neptune#DBCluster (Smithy: DescribeDBClusters, output shape DBClusterMessage, paginated items=DBClusters). Recorded behavior only: a cluster with no activity stream started reports DBActivityStreamStatus 'stopped' — a fixed enumerated state, not a generated value — alongside AutoMinorVersionUpgrade 'false', DocdbAnalyticsEnabled 'false', CustomDataGroupPresent 'false' and the regional HostedZoneId. Emulator: DBActivityStreamStatus, DocdbAnalyticsEnabled and CustomDataGroupPresent are absent, AutoMinorVersionUpgrade is 'true', HostedZoneId is the synthesised value. Why: the describe is the read-back a client polls after a create or modify; a client that checks whether a database activity stream is running cannot distinguish 'stopped' from 'the field is not implemented', and the read-back disagrees with what AWS would have written. Fix: the shared DBCluster projection used by the DescribeDBClusters handler (same projection as causes 1 and 3). Traces: neptune-db-cluster-lifecycle-and-custom-endpoint#task#2.

Cause 3 of 5: ModifyDBCluster's DBCluster loses the cluster's custom endpoints, along with the same omitted members and inverted default. AWS: ModifyDBCluster returns the modified cluster as a neptune#DBCluster. Recorded behavior only for the member: after a custom endpoint has been created against the cluster, the cluster report carries CustomEndpoints as a list with one member per custom endpoint, each the endpoint's DNS name (the name is derived from the endpoint and cluster identifiers and the region, so only its PRESENCE and count are the requirement, not the recorded hostname). The same AutoMinorVersionUpgrade 'false', DocdbAnalyticsEnabled 'false', CustomDataGroupPresent 'false' and regional HostedZoneId apply. Emulator: CustomEndpoints comes back as an empty value (type_mismatch: expected a list with the endpoint's member, got ''), DocdbAnalyticsEnabled and CustomDataGroupPresent are absent, AutoMinorVersionUpgrade is 'true', HostedZoneId is synthesised. Why: this is a loss of state the emulator itself created — a client that creates a custom endpoint and then reads the cluster to confirm it, or that enumerates a cluster's endpoints in order to connect through one, is told the cluster has none. Fix: the ModifyDBCluster handler's DBCluster projection and the cluster state that records custom endpoints created against the cluster. Traces: neptune-db-cluster-lifecycle-and-custom-endpoint#task#9.

Cause 4 of 5: CreateDBClusterSnapshot omits CustomDataGroupPresent from the snapshot it returns. AWS: the operation returns a com.amazonaws.neptune#DBClusterSnapshot. Recorded behavior only: a newly created snapshot reports CustomDataGroupPresent 'false'. Emulator: the member is absent. Why: a client that mirrors the snapshot record, or that compares a snapshot fetched from the emulator with one fetched from AWS, sees a structurally different snapshot. Fix: the CreateDBClusterSnapshot handler's DBClusterSnapshot projection. Traces: neptune-db-cluster-snapshot-lifecycle#task#1.

Cause 5 of 5: DescribeDBClusterSnapshots omits FullSnapshotSizeInBytes. AWS: the same DBClusterSnapshot is returned from DescribeDBClusterSnapshots. Recorded behavior only: the snapshot reports FullSnapshotSizeInBytes '0' for a snapshot of an empty cluster — the member is always present, its value being the snapshot's measured full size, so for an empty cluster it is 0. Emulator: the member is absent. Why: a client that reports or budgets snapshot storage reads nothing where AWS always reports a number, and cannot distinguish 'zero bytes' from 'unknown'. Fix: the DescribeDBClusterSnapshots handler's DBClusterSnapshot projection and the snapshot size recorded in state. Traces: neptune-db-cluster-snapshot-lifecycle#task#3.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true, bootstrap_signal=false, 181 steps, 110 passed, 69 mismatched, score 60.8%, 45 divergence clusters, all service=neptune. The five clusters below carry owned_by=[] and stub_path="".

Cause 1 — neptune-db-cluster-lifecycle-and-custom-endpoint#task#1, CreateDBCluster, expected 200 / actual 200, verdict mismatch. divergence_key service=neptune|CreateDBCluster|status=200->200. $.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.AutoMinorVersionUpgrade: value_mismatch (expected 'false', got 'true'); .DocdbAnalyticsEnabled: missing (expected 'false', got None); .CustomDataGroupPresent: missing (expected 'false', got None); .HostedZoneId: value_mismatch (expected the engine's regional hosted-zone id 'Z22EZDAJT6W257', got the synthesised 'ZT6579J0SIQMOV').

Cause 2 — neptune-db-cluster-lifecycle-and-custom-endpoint#task#2, DescribeDBClusters, 200 / 200. DBClusters.DBCluster.DBActivityStreamStatus: missing (expected 'stopped', got None); .AutoMinorVersionUpgrade: value_mismatch (expected 'false', got 'true'); .DocdbAnalyticsEnabled: missing (expected 'false'); .CustomDataGroupPresent: missing (expected 'false'); .HostedZoneId: value_mismatch (expected 'Z22EZDAJT6W257', got 'ZT6579J0SIQMOV').

Cause 3 — neptune-db-cluster-lifecycle-and-custom-endpoint#task#9, ModifyDBCluster, 200 / 200. ModifyDBClusterResult.DBCluster.CustomEndpoints: type_mismatch (expected {'member': '<endpoint>.cluster-custom-<cluster-resource-id>.<region>.neptune.amazonaws.com'}, got ''); .AutoMinorVersionUpgrade: value_mismatch (expected 'false', got 'true'); .DocdbAnalyticsEnabled and .CustomDataGroupPresent: missing (expected 'false'); .HostedZoneId: value_mismatch as above.

Cause 4 — neptune-db-cluster-snapshot-lifecycle#task#1, CreateDBClusterSnapshot, 200 / 200. CreateDBClusterSnapshotResult.DBClusterSnapshot.CustomDataGroupPresent: missing (expected 'false', got None).

Cause 5 — neptune-db-cluster-snapshot-lifecycle#task#3, DescribeDBClusterSnapshots, 200 / 200. DescribeDBClusterSnapshotsResult.DBClusterSnapshots.DBClusterSnapshot.FullSnapshotSizeInBytes: missing (expected '0', got None).

## Steps to Reproduce
1. Clone the repo, boot the emulator built from it, and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay neptune-db-cluster-lifecycle-and-custom-endpoint step task#1 (CreateDBCluster for the Neptune engine) and compare DBCluster.AutoMinorVersionUpgrade, .DocdbAnalyticsEnabled, .CustomDataGroupPresent and .HostedZoneId with the recording.
3. Cause 2: replay the same fixture step task#2 (DescribeDBClusters for that cluster) and additionally check DBCluster.DBActivityStreamStatus.
4. Cause 3: replay the same fixture through step task#9 — create a custom endpoint against the cluster, then ModifyDBCluster — and check that ModifyDBClusterResult.DBCluster.CustomEndpoints lists the created endpoint rather than coming back empty.
5. Cause 4: replay neptune-db-cluster-snapshot-lifecycle step task#1 (CreateDBClusterSnapshot) and check DBClusterSnapshot.CustomDataGroupPresent.
6. Cause 5: replay the same fixture step task#3 (DescribeDBClusterSnapshots) and check DBClusterSnapshot.FullSnapshotSizeInBytes is present.
