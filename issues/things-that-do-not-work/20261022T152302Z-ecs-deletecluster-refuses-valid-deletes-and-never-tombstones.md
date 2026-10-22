Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-cluster-capacity-provider-lifecycle
Issue-Step: teardown#16
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-cluster-capacity-provider-lifecycle ecs-err-run-task-fargate-on-ec2-task-definition ecs-err-service-not-active ecs-service-tag-empty-string-vs-absent-key-preserved ecs-task-plane-reads-and-launch-errors ecs-taskset-create-describe-update-delete-lifecycle ecs-taskset-crud-lifecycle ecs-taskset-error-paths ecs-taskset-lifecycle ecs-taskset-multiple-primary-switch ecs-taskset-primary-promotion ecs-taskset-tags-and-external-id ecs-taskset-tags-and-include-fields ecs-err-delete-cluster-contains-services ecs-service-lifecycle-on-cluster ecs-task-set-external-deployment-workflow ecs-taskset-basic-lifecycle ecs-taskset-errors ecs-taskset-lifecycle-ec2 ecs-run-task-and-task-protection ecs-capacity-provider-management ecs-update-nonexistent-cluster-errors ecs-cluster-create-and-settings-update ecs-cluster-create-describe-list ecs-cluster-describe-and-list-lifecycle ecs-cluster-error-paths ecs-cluster-lifecycle-settings-update ecs-cluster-multi-isolation ecs-cluster-read-and-update-lifecycle ecs-cluster-settings-lifecycle ecs-describe-clusters-missing-cluster-reports-failure ecs-describe-clusters-partial-failure ecs-describe-clusters-valid-and-missing ecs-list-describe-multiple-clusters
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|DeleteCluster|2xx->4xx|status
Issue-Key: ecs|DeleteCluster|2xx->4xx|$.cluster
Issue-Key: ecs|DeleteCluster|2xx->4xx|$.clusterCount
Issue-Key: ecs|DescribeClusters|2xx->2xx|$.clusters[*].pendingTasksCount
Issue-Key: ecs|DescribeClusters|2xx->2xx|$.clusters[*].settings
Issue-Key: ecs|DescribeClusters|2xx->2xx|$.clusters[*].statistics
Issue-Key: ecs|DescribeClusters|2xx->2xx|$.clusters[*].status
Issue-Key: ecs|DescribeClusters|2xx->2xx|$.clusters[*].tags
Issue-Key: ecs|DeleteCluster|4xx->4xx|$.message
Issue-Related: 20261022T152302Z-ecs-cluster-mutations-are-not-persisted-settings-configuration.md
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# ECS DeleteCluster refuses valid deletes and never tombstones the cluster as INACTIVE

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCluster

## Description
Cluster deletion and cluster reads diverge as a pair: the emulator refuses deletes AWS accepts (because drained services/tasks are still counted against the cluster), and when a delete does succeed it removes the cluster outright instead of leaving an INACTIVE tombstone, so the follow-up DescribeClusters reports a MISSING failure where AWS returns the cluster with status INACTIVE. The same DescribeClusters projection also omits the empty settings/statistics/tags lists and does not preserve the order of the requested cluster identifiers.

THIS BUNDLE CONTAINS 8 CAUSES.

Cause 1 of 8 — DeleteCluster answers 400 where AWS answers 200.
AWS: Smithy: DeleteCluster errors are ClusterContainsServicesException / ClusterContainsTasksException / ClusterContainsContainerInstancesException / ClusterContainsCapacityProviderException (all client, HTTP 400). Recording: after the fixture has deleted its services/task sets, DeleteCluster answers 200 with the cluster.
Emulator: answers HTTP 400 — the drained services and task sets are still counted, so the delete is refused.
Why: every teardown that deletes its services and then its cluster fails, and clients cannot release a cluster they have legitimately emptied. 13 replayed steps are in this cluster alone.
Fix: DeleteCluster's contains-resources precondition in services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/delete_cluster.rs — it must ignore services/tasks already in INACTIVE/STOPPED state.
Traces: ecs-cluster-capacity-provider-lifecycle#teardown#16 (+12 members, e.g. ecs-taskset-lifecycle#teardown#16).

Cause 2 of 8 — DeleteCluster still 400 after the contained service is deleted.
AWS: recording: 200 with cluster status INACTIVE and clusterCount present.
Emulator: 400; no cluster body at all.
Why: the documented 'delete the services, then the cluster' sequence is unusable.
Fix: as Cause 1.
Traces: ecs-err-delete-cluster-contains-services#task#8 (+5 members, e.g. ecs-service-lifecycle-on-cluster#teardown#14).

Cause 3 of 8 — DeleteCluster 400 after the cluster's tasks were stopped.
AWS: recording: 200, cluster returned with status INACTIVE and settings=[{containerInsights, disabled}].
Emulator: 400, no cluster.
Fix: as Cause 1, for the task-count precondition.
Traces: ecs-run-task-and-task-protection#teardown#12.

Cause 4 of 8 — DescribeClusters after a successful delete reports MISSING instead of the INACTIVE cluster.
AWS: Smithy: Cluster.status documents INACTIVE as 'The cluster has been deleted. Clusters with an INACTIVE status may remain discoverable in your account for a period of time.' Recording: clusters=[{status: INACTIVE, ...}], failures=[].
Emulator: clusters=[] and failures=[{arn: <cluster arn>, reason: 'MISSING'}]; status 200 both sides.
Why: a client that confirms deletion by describing the cluster gets a MISSING failure, which it must treat as 'never existed' rather than 'deleted'.
Fix: DeleteCluster must retain the record with status INACTIVE (services/.../src/state.rs) and DescribeClusters must return it.
Traces: ecs-capacity-provider-management#teardown#9 (+1 member, ecs-update-nonexistent-cluster-errors#teardown#6).

Cause 5 of 8 — a deleted cluster is still reported ACTIVE with a stale pending-task count.
AWS: recording: clusters[0].status INACTIVE, pendingTasksCount 0, settings=[], statistics=[], tags=[].
Emulator: status 'ACTIVE', pendingTasksCount 1, and settings/statistics/tags omitted.
Why: deletion is invisible to readers and the task counters never settle, so clients poll forever for a drained cluster.
Fix: DeleteCluster state transition plus the task counters that back pendingTasksCount.
Traces: ecs-run-task-and-task-protection#teardown#13.

Cause 6 of 8 — DescribeClusters omits the empty settings, statistics and tags lists.
AWS: Smithy: DescribeClusters -> clusters : Clusters. Recording: an ACTIVE cluster read without include carries settings=[], statistics=[], tags=[] alongside the counts.
Emulator: those three members are absent from the cluster object (status 200 both sides).
Why: clients that index settings/tags without a null check break, and 'absent' is not distinguishable from 'empty'.
Fix: the DescribeClusters cluster projection in services/.../src/ops/describe_clusters.rs.
Traces: ecs-cluster-capacity-provider-lifecycle#task#2 (+12 members, e.g. ecs-describe-clusters-partial-failure#task#2).

Cause 7 of 8 — DescribeClusters does not preserve the requested cluster order.
AWS: recording: clusters are returned in the order the identifiers were supplied (cluster-b then cluster-a).
Emulator: returns them in its own stored order (cluster-a then cluster-b); the objects themselves otherwise match.
Why: a client that zips the request list against the response list pairs the wrong cluster with the wrong identifier.
Fix: DescribeClusters' result ordering in services/.../src/ops/describe_clusters.rs.
Traces: ecs-cluster-multi-isolation#task#4 (+1 member, ecs-list-describe-multiple-clusters#task#4).

Cause 8 of 8 — ClusterContainsServicesException message is a bespoke interpolated sentence.
AWS: Smithy: ClusterContainsServicesException (client, HTTP 400). Recording: message 'The Cluster cannot be deleted while Services are active.' — a fixed sentence that does not name the cluster.
Emulator: status 400 and the right condition, but message 'cluster "<name>" still contains services that have not been deleted'.
Why: clients and test suites that match the documented fixed sentence (and log scrapers keyed on it) do not recognise the refusal; the rule here is that ECS returns a constant sentence for this condition rather than one built from the request.
Fix: the error construction for the contains-services refusal in services/.../src/ops/delete_cluster.rs.
Traces: ecs-err-delete-cluster-contains-services#task#5.

## Evidence
Cause 1 (ecs-cluster-capacity-provider-lifecycle#teardown#16, DeleteCluster): status: expected 200, got 400 (13 clustered steps).
Cause 2 (ecs-err-delete-cluster-contains-services#task#8): status expected 200, got 400; $.cluster missing (expected cluster with status 'INACTIVE'); $.clusterCount missing.
Cause 4 (ecs-capacity-provider-management#teardown#9, DescribeClusters, 200/200): $.clusters[0] missing (expected cluster with 'status': 'INACTIVE'), $.failures[0] extra (got {'arn': ..., 'reason': 'MISSING'}).
Cause 5 (ecs-run-task-and-task-protection#teardown#13): $.clusters[0].status: value_mismatch (expected 'INACTIVE', got 'ACTIVE'); $.clusters[0].pendingTasksCount: expected 0, got 1; $.clusters[0].settings/statistics/tags: missing (expected []).
Cause 6 (ecs-cluster-capacity-provider-lifecycle#task#2): $.clusters[0] missing — expected object carries 'settings': [], 'statistics': [], 'tags': []; emulator's object has none of the three.
Cause 7 (ecs-cluster-multi-isolation#task#4): clusters[0]/clusters[1] swapped relative to the requested identifiers.
Cause 8 (ecs-err-delete-cluster-contains-services#task#5): $.message: value_mismatch (expected 'The Cluster cannot be deleted while Services are active.', got 'cluster "..." still contains services that have not been deleted').

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus (replay_wire_corpus).
2. Fixture ecs-cluster-capacity-provider-lifecycle step teardown#16 and ecs-err-delete-cluster-contains-services steps task#5/task#8.
3. By hand: CreateCluster; CreateService; DeleteService; DeleteCluster -> emulator answers 400 instead of 200. On a cluster that does delete: DescribeClusters{clusters:[name]} -> failures MISSING instead of a cluster with status INACTIVE.
4. DescribeClusters{clusters:[b, a]} on two clusters -> response order does not match the request order.
