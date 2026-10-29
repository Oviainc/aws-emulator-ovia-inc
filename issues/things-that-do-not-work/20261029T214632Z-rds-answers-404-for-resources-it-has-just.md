Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-cluster-lifecycle-with-snapshot
Issue-Step: task#4
Issue-Severity: CRITICAL
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-cluster-lifecycle-with-snapshot rds-global-cluster-delete-with-member rds-global-cluster-mutation-envelopes rds-db-parameter-group-lifecycle rds-db-parameter-group-lifecycle-and-engine-defaults rds-db-snapshot-and-export-task-error-paths rds-export-task-lifecycle
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|CreateDBClusterSnapshot|2xx->4xx|$.CreateDBClusterSnapshotResponse
Issue-Key: rds|CreateDBClusterSnapshot|2xx->4xx|status
Issue-Key: rds|DescribeDBClusterSnapshots|2xx->4xx|$
Issue-Key: rds|DescribeDBClusterSnapshots|2xx->4xx|$.DescribeDBClusterSnapshotsResponse
Issue-Key: rds|DescribeDBClusterSnapshots|2xx->4xx|$.DescribeDBClusterSnapshotsResponse.DescribeDBClusterSnapshotsResult.DBClusterSnapshots.DBClusterSnapshot.Status
Issue-Key: rds|DescribeDBClusterSnapshots|2xx->4xx|status
Issue-Key: rds|DeleteDBClusterSnapshot|2xx->4xx|$.DeleteDBClusterSnapshotResponse
Issue-Key: rds|DeleteDBClusterSnapshot|2xx->4xx|status
Issue-Key: rds|DeleteDBCluster|2xx->4xx|status
Issue-Key: rds|DeleteGlobalCluster|2xx->4xx|status
Issue-Key: rds|AddTagsToResource|2xx->4xx|$.AddTagsToResourceResponse
Issue-Key: rds|AddTagsToResource|2xx->4xx|status
Issue-Key: rds|ListTagsForResource|2xx->4xx|$.ListTagsForResourceResponse
Issue-Key: rds|ListTagsForResource|2xx->4xx|status
Issue-Key: rds|DeleteGlobalCluster|2xx->4xx|$.DeleteGlobalClusterResponse
Issue-Key: rds|DeleteDBParameterGroup|2xx->4xx|status
Issue-Key: rds|DescribeExportTasks|2xx->4xx|$.DescribeExportTasksResponse
Issue-Key: rds|DescribeExportTasks|2xx->4xx|status
Issue-Commit: 9919ffd222ff0dfbfa1095fff748926e9311733c

# RDS answers 404 for resources it has just created, breaking snapshot, tag and delete flows

> _Note: this issue was identified against the code base at commit `9919ffd222ff0dfbfa1095fff748926e9311733c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBClusterSnapshot

## Description
Ten steps that AWS answers 200 are refused by the emulator, almost always 404, for a resource the same fixture created moments earlier. The whole cluster-snapshot path never persists anything, global-cluster ARNs are not resolvable for tagging or deletion, and a describe with an empty result set is turned into a not-found. 10 causes.

Cause 1 of 10: CreateDBClusterSnapshot on an existing cluster.
AWS: 200 with CreateDBClusterSnapshotResult.DBClusterSnapshot (Status 'creating', PercentProgress 0, the cluster's AvailabilityZones, EngineVersion, DBClusterSnapshotArn, DbClusterResourceId).
Emulator: 400 with no CreateDBClusterSnapshotResponse at all.
Why: manual cluster snapshots cannot be taken, so every backup/restore flow is unreachable.
Fix: the CreateDBClusterSnapshot handler and the cluster-snapshot state.
Traces: rds-db-cluster-lifecycle-with-snapshot#task#4.

Cause 2 of 10: DescribeDBClusterSnapshots polling the snapshot to 'available'.
AWS: 200 with the snapshot at Status 'available', PercentProgress 100.
Emulator: 404, Code DBClusterSnapshotNotFoundFault, 'DBClusterSnapshot not found: <id>' — the poll ran to the 60 s cap and never converged (a consequence of cause 1, but a separate read path).
Why: a client waiting on snapshot availability hangs to its timeout.
Fix: the DescribeDBClusterSnapshots handler and the cluster-snapshot state.
Traces: rds-db-cluster-lifecycle-with-snapshot#task#5.

Cause 3 of 10: DeleteDBClusterSnapshot.
AWS: 200 with DeleteDBClusterSnapshotResult.DBClusterSnapshot.
Emulator: 404, no response envelope.
Why: teardown of a snapshot is impossible.
Fix: the DeleteDBClusterSnapshot handler.
Traces: rds-db-cluster-lifecycle-with-snapshot#teardown#8.

Cause 4 of 10: DeleteDBCluster for a cluster that is a global-cluster member.
AWS: 200.
Emulator: 404.
Why: the member cluster cannot be torn down, leaking state across fixtures.
Fix: the DeleteDBCluster handler's lookup for global-cluster member clusters.
Traces: rds-global-cluster-delete-with-member#teardown#9.

Cause 5 of 10: DeleteGlobalCluster at teardown.
AWS: 200.
Emulator: 404.
Why: the global cluster cannot be deleted.
Fix: the DeleteGlobalCluster handler's lookup.
Traces: rds-global-cluster-delete-with-member#teardown#11.

Cause 6 of 10: AddTagsToResource with a global-cluster ARN.
AWS: 200 with AddTagsToResourceResponse carrying only ResponseMetadata.
Emulator: 404.
Why: global clusters cannot be tagged, so tag-based discovery never sees them.
Fix: the AddTagsToResource handler's ARN-to-resource resolution for global clusters.
Traces: rds-global-cluster-mutation-envelopes#task#8.

Cause 7 of 10: ListTagsForResource with a global-cluster ARN.
AWS: 200 with ListTagsForResourceResult.TagList.
Emulator: 404.
Why: same resolution gap on the read side.
Fix: the ListTagsForResource handler's ARN resolution.
Traces: rds-global-cluster-mutation-envelopes#task#9.

Cause 8 of 10: DeleteGlobalCluster on an empty global cluster created in the same fixture.
AWS: 200 with DeleteGlobalClusterResult.GlobalCluster (Status 'available', its Endpoint, TagList and GlobalClusterArn).
Emulator: 404, no envelope.
Why: the global-cluster lifecycle cannot complete.
Fix: the DeleteGlobalCluster handler's lookup.
Traces: rds-global-cluster-mutation-envelopes#task#10.

Cause 9 of 10: DeleteDBParameterGroup at teardown for a group created in the fixture.
AWS: 200.
Emulator: 404.
Why: parameter groups cannot be deleted, so the group leaks and later creates collide.
Fix: the DeleteDBParameterGroup handler's lookup.
Traces: rds-db-parameter-group-lifecycle#teardown#11; also rds-db-parameter-group-lifecycle-and-engine-defaults#teardown#17.

Cause 10 of 10: DescribeExportTasks filtered on an export task that does not exist.
AWS: 200 with DescribeExportTasksResult.ExportTasks serialized as the empty value — a filter that matches nothing is an empty list, not an error.
Emulator: 404.
Why: a client enumerating export tasks must special-case a 404 AWS never sends.
Fix: the DescribeExportTasks handler — an unmatched filter returns an empty collection.
Traces: rds-db-snapshot-and-export-task-error-paths#task#12; also rds-export-task-lifecycle#task#2.

## Evidence
Replay cluster details (expected status vs actual):
1. rds-db-cluster-lifecycle-with-snapshot#task#4 CreateDBClusterSnapshot: expected 200 got 400; $.CreateDBClusterSnapshotResponse missing (recording carried DBClusterSnapshot with Status 'creating', PercentProgress '0', SnapshotType 'manual', DBClusterSnapshotArn arn:aws:rds:us-west-2:000000000000:cluster-snapshot:...).
2. #task#5 DescribeDBClusterSnapshots: expected 200 got 404; detail notes "poll capped at 60s" and the actual body was ErrorResponse Code DBClusterSnapshotNotFoundFault 'DBClusterSnapshot not found: sim2real-probe-rds-rds-cluster-snap-2d5d17ef'.
3. #teardown#8 DeleteDBClusterSnapshot: expected 200 got 404; $.DeleteDBClusterSnapshotResponse missing.
4. rds-global-cluster-delete-with-member#teardown#9 DeleteDBCluster: expected 200 got 404.
5. rds-global-cluster-delete-with-member#teardown#11 DeleteGlobalCluster: expected 200 got 404.
6. rds-global-cluster-mutation-envelopes#task#8 AddTagsToResource: expected 200 got 404; $.AddTagsToResourceResponse missing.
7. rds-global-cluster-mutation-envelopes#task#9 ListTagsForResource: expected 200 got 404; $.ListTagsForResourceResponse missing (recording carried TagList with the fixture's tag).
8. rds-global-cluster-mutation-envelopes#task#10 DeleteGlobalCluster: expected 200 got 404; $.DeleteGlobalClusterResponse missing.
9. rds-db-parameter-group-lifecycle#teardown#11 DeleteDBParameterGroup: expected 200 got 404.
10. rds-db-snapshot-and-export-task-error-paths#task#12 DescribeExportTasks: expected 200 got 404; $.DescribeExportTasksResponse missing (recording: DescribeExportTasksResult.ExportTasks '').

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Re-run rds-db-cluster-lifecycle-with-snapshot#task#4 and every step in trace_ids; each fixture creates the resource in an earlier step and the failing step reads, tags or deletes it.
