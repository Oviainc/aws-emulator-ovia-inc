Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-err-run-task-fargate-on-ec2-task-definition
Issue-Step: teardown#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-err-run-task-fargate-on-ec2-task-definition ecs-err-run-task-unknown-platform-version ecs-task-plane-reads-and-launch-errors ecs-cluster-create-describe-list ecs-cluster-multi-isolation ecs-container-instance-and-attribute-errors ecs-run-task-and-task-protection
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|DeleteCluster|2xx->4xx|status
Issue-Key: ecs|DescribeClusters|2xx->2xx|$.clusters[*]
Issue-Key: ecs|CreateCluster|2xx->2xx|$.clusterCount
Issue-Key: ecs|DeleteCluster|2xx->2xx|$.clusterCount
Issue-Key: ecs|DeleteCluster|2xx->2xx|$.cluster.registeredContainerInstancesCount
Issue-Key: ecs|DeleteCluster|2xx->4xx|$.cluster
Issue-Key: ecs|DescribeClusters|2xx->2xx|$.clusters[*].pendingTasksCount
Issue-Key: ecs|DescribeClusters|2xx->2xx|$.clusters[*].status
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# ECS cluster lifecycle and DescribeClusters body diverge: deleted clusters stay ACTIVE and DeleteCluster 400s

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCluster

## Description
Seven distinct cluster-plane divergences in the ecs emulator, all in the cluster handler/state and the Cluster serializer. A fixer must close ALL SEVEN causes below before submitting.

Cause 1 of 7: DeleteCluster rejects a teardown that AWS accepts.
AWS: Smithy DeleteCluster -> DeleteClusterResponse{cluster: Cluster}, errors include ClusterContainsTasksException/ClusterContainsContainerInstancesException (httpError 400). In the recording the teardown DeleteCluster answers 200 with the Cluster body, so no task or instance was holding the cluster.
Emulator: answers HTTP 400 for the same request.
Why: a client's teardown step fails, the cluster is never released, and every subsequent step in the session operates against a cluster that should no longer exist.
Fix: DeleteCluster handler/state in the ecs service crate. Hypothesis: the emulator holds a phantom task for the cluster and raises ClusterContainsTasksException; the phantom comes from RunTask answering 200 where AWS answers 400 (already open as 20261022T152302Z-ecs-accepts-invalid-requests-with-200-and-misreports.md, which does NOT carry these teardown replay keys). DeleteCluster's own acceptance must be re-verified once that is fixed.
Traces: ecs-err-run-task-fargate-on-ec2-task-definition#teardown#7 (also ecs-err-run-task-unknown-platform-version#teardown#8, ecs-task-plane-reads-and-launch-errors#teardown#9).

Cause 2 of 7: the same DeleteCluster rejection, and with no cluster body at all.
AWS: 200 with cluster.status INACTIVE (Cluster.status models INACTIVE as 'the cluster has been deleted').
Emulator: 400, and $.cluster is absent from the response.
Why: a client cannot observe the delete transition; it sees neither the INACTIVE cluster nor a modeled success.
Fix: DeleteCluster handler/state.
Traces: ecs-run-task-and-task-protection#teardown#12.

Cause 3 of 7: a deleted cluster is still reported ACTIVE with live task counters.
AWS: after DeleteCluster, DescribeClusters reports status INACTIVE and pendingTasksCount 0 (Smithy Cluster.status: INACTIVE == deleted; pendingTasksCount == tasks in PENDING state).
Emulator: status 'ACTIVE' and pendingTasksCount 1.
Why: a client that polls DescribeClusters to confirm teardown never sees the cluster leave ACTIVE, so delete-then-recreate and idempotent cleanup loops never terminate.
Fix: DeleteCluster state transition + DescribeClusters projection.
Traces: ecs-run-task-and-task-protection#teardown#13.

Cause 4 of 7: DeleteCluster succeeds while still reporting registered container instances.
AWS: the recorded DeleteCluster response carries cluster.registeredContainerInstancesCount 0 (Smithy: 'the number of container instances registered into the cluster'); a cluster that really had one would instead be refused with ClusterContainsContainerInstancesException.
Emulator: 200 with registeredContainerInstancesCount 1 — it neither clears the counter nor raises the modeled error, which is also inconsistent with Cause 1 where it does refuse for tasks.
Why: the deleted cluster reports phantom capacity, so clients believe instances are still registered against a cluster that is gone.
Fix: DeleteCluster handler/state (container-instance bookkeeping).
Traces: ecs-container-instance-and-attribute-errors#teardown#14.

Cause 5 of 7: cluster inventory count is one too high after CreateCluster.
AWS: recorded behavior only — the step's asserted $.clusterCount is 1.
Emulator: 2.
Why: a client enumerating its clusters sees an extra cluster it did not create, so count-based assertions and cleanup sweeps touch the wrong set.
Fix: cluster state/listing in the ecs service crate. Hypothesis: a default or previously created cluster is left in the inventory the response counts.
Traces: ecs-cluster-multi-isolation#task#2.

Cause 6 of 7: a deleted cluster is still counted.
AWS: recorded behavior only — after DeleteCluster the asserted $.clusterCount is 0.
Emulator: 1.
Why: the cluster inventory never shrinks on delete, so clients cannot tell a deleted cluster from a live one by enumeration.
Fix: cluster state removal/tombstoning on DeleteCluster.
Traces: ecs-cluster-multi-isolation#task#7.

Cause 7 of 7: DescribeClusters omits most of the Cluster structure.
AWS: Smithy Cluster declares attachments, capacityProviders, defaultCapacityProviderStrategy, settings, tags and statistics; the recording returns all of them (settings [{containerInsights, disabled}], tags as requested, empty lists for attachments/capacityProviders/defaultCapacityProviderStrategy) and a statistics list that also carries the External and ManagedInstances task counters and the draining service counters.
Emulator: returns only clusterArn, clusterName, status, the four count members and a truncated statistics list — attachments, capacityProviders, defaultCapacityProviderStrategy, settings and tags are absent and several statistics entries are missing.
Why: a client reading cluster settings (Container Insights), tags, or the external/managed-instance statistics gets null/absent where AWS always returns a value, so typed SDK accessors and tag-driven logic break.
Fix: the DescribeClusters serializer / Cluster projection (include=ATTACHMENTS,SETTINGS,STATISTICS,TAGS handling).
Traces: ecs-cluster-create-describe-list#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline.
Cause 1: DeleteCluster expected status 200, actual 400 (divergence_key service=ecs|DeleteCluster|status=200->400|keys=status; 3 affected steps).
Cause 2: DeleteCluster expected 200, actual 400; detail '$.cluster: missing (expected {...clusterName ..., status: INACTIVE, settings:[{containerInsights,disabled}], statistics: [], tags: []}, got None)'.
Cause 3: DescribeClusters 200/200; detail '$.clusters[0].pendingTasksCount: value_mismatch (expected 0, got 1)' and '$.clusters[0].status: value_mismatch (expected INACTIVE, got ACTIVE)'.
Cause 4: DeleteCluster 200/200; detail '$.cluster.registeredContainerInstancesCount: value_mismatch (expected 0, got 1)'.
Cause 5: 200/200; detail '$.clusterCount: value_mismatch (expected 1, got 2)'.
Cause 6: 200/200; detail '$.clusterCount: value_mismatch (expected 0, got 1)'.
Cause 7: DescribeClusters 200/200; detail '$.clusters[0]: missing (expected {activeServicesCount, attachments: [], capacityProviders: [], clusterArn, clusterName, defaultCapacityProviderStrategy: [], pendingTasksCount, registeredContainerInstancesCount, runningTasksCount, settings: [{name: containerInsights, value: disabled}], statistics: [... runningExternalTasksCount, pendingExternalTasksCount, runningManagedInstancesTasksCount, pendingManagedInstancesTasksCount, activeExternalServiceCount, drainingExternalServiceCount, activeManagedInstancesServiceCount, drainingManagedInstancesServiceCount], status: ACTIVE, tags: [{key: purpose, value: probe}]}, got {clusterArn, clusterName, status, registeredContainerInstancesCount, runningTasksCount, pendingTasksCount, activeServicesCount, statistics: [runningEC2TasksCount, runningFargateTasksCount, pendingEC2TasksCount, pendingFargateTasksCount, activeEC2ServiceCount, activeFargateServ...]})'.
state_reset_failures = 0 for this run, so none of these ran against a previous fixture's state.

## Steps to Reproduce
1. Build the emulator from mainline and boot it.
2. Replay fixture ecs-err-run-task-fargate-on-ec2-task-definition and compare step teardown#7 (DeleteCluster) — representative.
3. Replay the other bundled steps: ecs-cluster-create-describe-list#task#2, ecs-cluster-multi-isolation#task#2, ecs-cluster-multi-isolation#task#7, ecs-container-instance-and-attribute-errors#teardown#14, ecs-run-task-and-task-protection#teardown#12, ecs-run-task-and-task-protection#teardown#13.
4. All seven must match the recorded status and body before this issue is closed.
