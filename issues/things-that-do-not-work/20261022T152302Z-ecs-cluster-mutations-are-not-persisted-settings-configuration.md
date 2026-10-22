Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-cluster-capacity-provider-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-cluster-capacity-provider-lifecycle ecs-cluster-lifecycle-settings-update ecs-cluster-multi-isolation ecs-cluster-update-generic ecs-cluster-read-and-update-lifecycle ecs-cluster-update-configuration-lifecycle ecs-container-instance-and-attribute-errors
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|DescribeClusters|2xx->2xx|$.clusters[*]
Issue-Key: ecs|DescribeClusters|2xx->2xx|$.clusters[*].settings[*].value
Issue-Key: ecs|DeleteCluster|4xx->2xx|$.__type
Issue-Key: ecs|DeleteCluster|4xx->2xx|$.message
Issue-Key: ecs|DeleteCluster|4xx->2xx|status
Issue-Key: ecs|DescribeClusters|2xx->2xx|$
Issue-Key: ecs|DescribeClusters|2xx->2xx|$.clusters[*].attachmentsStatus
Issue-Key: ecs|DescribeClusters|2xx->2xx|$.failures[*]
Issue-Key: ecs|DeleteCluster|2xx->2xx|$.cluster.settings[*].value
Issue-Key: ecs|DeleteCluster|2xx->2xx|$.cluster.registeredContainerInstancesCount
Issue-Related: 20261022T152302Z-ecs-cluster-responses-omit-settings-configuration-and-clustercount.md
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# ECS cluster mutations are not persisted: settings, configuration, capacity-provider attachments and counts

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusters

## Description
Cluster-mutating operations (UpdateClusterSettings, UpdateCluster, PutClusterCapacityProviders, container-instance registration/deregistration) are accepted, but the emulator's cluster record keeps its creation-time values: settings never change, configuration is never stored, capacity-provider attachments never materialise, and the registered-instance count is never decremented. Because the state never moves, the busy-state refusal AWS raises while attachments update is never raised either.

THIS BUNDLE CONTAINS 8 CAUSES.

Cause 1 of 8 — containerInsights set to 'enabled' is never reflected in DescribeClusters.
AWS: Smithy: Cluster.settings : ClusterSettings ("This parameter indicates whether CloudWatch Container Insights is on or off for a cluster"); UpdateClusterSettings persists the value and DescribeClusters(include=[SETTINGS]) returns it. Recording: settings=[{containerInsights, enabled}].
Emulator: DescribeClusters keeps returning {containerInsights, disabled} — the replay polled 30 times over the step's wait budget and the value never changed. Status 200 both sides.
Why: a client that enables Container Insights and then waits for the setting to take effect waits forever and concludes the API silently ignored the write.
Fix: the UpdateClusterSettings handler's write into the stored cluster (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/state.rs) and the DescribeClusters read of it.
Traces: ecs-cluster-capacity-provider-lifecycle#task#5.

Cause 2 of 8 — DeleteCluster does not raise UpdateInProgressException while cluster attachments are updating.
AWS: Smithy: DeleteCluster errors include UpdateInProgressException (client, HTTP 400). Recording: status 400, __type 'UpdateInProgressException', message 'The specified cluster is in a busy state. Cluster attachments must be in UPDATE_COMPLETE or UPDATE_FAILED state before they can be updated. Wait and try again.'
Emulator: answers 200 with a normal delete response; no __type, no message.
Why: a client that relies on the busy-state refusal to back off instead believes the delete succeeded while the capacity-provider association is still mid-flight.
Fix: DeleteCluster's precondition check (services/.../src/ops/delete_cluster.rs) over the cluster's attachmentsStatus.
Traces: ecs-cluster-capacity-provider-lifecycle#task#9.

Cause 3 of 8 — capacity-provider attachments never appear on the cluster.
AWS: Smithy: Cluster.attachments : Attachments ("When using a capacity provider with a cluster, the capacity provider and associated resources are returned as cluster attachments") and Cluster.attachmentsStatus (UPDATE_IN_PROGRESS -> UPDATE_COMPLETE). Recording: after PutClusterCapacityProviders the cluster reports capacityProviders=[FARGATE, FARGATE_SPOT], a two-entry defaultCapacityProviderStrategy, attachments=[] and attachmentsStatus=UPDATE_COMPLETE.
Emulator: attachmentsStatus is never emitted (polled 13 times, always None) and the cluster is reported as MISSING in failures instead of being returned at all.
Why: a client waiting for attachmentsStatus=UPDATE_COMPLETE before using the capacity provider never proceeds.
Fix: PutClusterCapacityProviders' state transition and the DescribeClusters ATTACHMENTS projection.
Traces: ecs-cluster-capacity-provider-lifecycle#task#10.

Cause 4 of 8 — cluster configuration and updated settings both lost on a settings-update lifecycle.
AWS: Smithy: Cluster.configuration : ClusterConfiguration; recording shows configuration={executeCommandConfiguration:{logging: DEFAULT}} and settings value 'enabled' after the update.
Emulator: configuration absent, settings value still 'disabled' (plus the fabricated statistics and echoed tags covered by the cluster-projection issue).
Why: neither the execute-command configuration nor the settings write is readable afterwards.
Fix: UpdateCluster / UpdateClusterSettings writes in services/.../src/state.rs.
Traces: ecs-cluster-lifecycle-settings-update#teardown#6.

Cause 5 of 8 — settings value crosses between clusters in one account.
AWS: recording: this cluster's settings value is 'disabled'.
Emulator: returns 'enabled' — the value belonging to the sibling cluster created in the same fixture.
Why: per-cluster settings are not keyed per cluster, so enabling Container Insights on one cluster appears to enable it on another.
Fix: the per-cluster settings keying in services/.../src/state.rs.
Traces: ecs-cluster-multi-isolation#task#8 (+1 member, ecs-cluster-update-generic#teardown#3).

Cause 6 of 8 — configuration and settings lost on the read-and-update lifecycle.
AWS: recording: configuration={executeCommandConfiguration:{logging: DEFAULT}}, settings value 'enabled'.
Emulator: configuration absent, settings value 'disabled'.
Fix: as Cause 4.
Traces: ecs-cluster-read-and-update-lifecycle#teardown#6.

Cause 7 of 8 — an OVERRIDE execute-command log configuration is accepted and then lost.
AWS: recording: configuration={executeCommandConfiguration:{logging: OVERRIDE, logConfiguration:{cloudWatchEncryptionEnabled: false, cloudWatchLogGroupName: /ecs/<cluster>, s3EncryptionEnabled: false}}} and settings=[{containerInsights, enabled}].
Emulator: configuration absent and settings absent.
Why: a client cannot verify the logging destination it configured, so execute-command auditing cannot be set up against the emulator.
Fix: UpdateCluster's configuration write and the cluster projection.
Traces: ecs-cluster-update-configuration-lifecycle#teardown#6.

Cause 8 of 8 — registeredContainerInstancesCount is never decremented after deregistration.
AWS: Smithy: Cluster.registeredContainerInstancesCount ("The number of container instances registered into the cluster. This includes container instances in both ACTIVE and DRAINING status"). Recording: after the instance is deregistered the count is 0.
Emulator: still reports 1.
Why: a client that waits for the cluster to drain to zero instances before deleting it never sees the count fall.
Fix: DeregisterContainerInstance's update of the owning cluster's counters in services/.../src/state.rs.
Traces: ecs-container-instance-and-attribute-errors#teardown#14.

## Evidence
Cause 1 (ecs-cluster-capacity-provider-lifecycle#task#5, DescribeClusters, 200/200):
  $.clusters[0].settings[0].value: poll: not in ['enabled'] after 30 attempt(s) (expected 'enabled', got 'disabled')
Cause 2 (ecs-cluster-capacity-provider-lifecycle#task#9, DeleteCluster): status expected 400, got 200; $.__type missing (expected 'UpdateInProgressException'); $.message missing (expected 'The specified cluster is in a busy state. ...').
Cause 3 (ecs-cluster-capacity-provider-lifecycle#task#10, DescribeClusters): $: poll capped at 60s; $.clusters[0].attachmentsStatus: poll: not in ['UPDATE_COMPLETE'] after 13 attempt(s) (expected 'UPDATE_COMPLETE', got None); $.failures[0] extra {'reason': 'MISSING'}.
Cause 5 (ecs-cluster-multi-isolation#task#8, DeleteCluster): $.cluster.settings[0].value: value_mismatch (expected 'disabled', got 'enabled').
Cause 7 (ecs-cluster-update-configuration-lifecycle#teardown#6, DeleteCluster): $.cluster.configuration missing (expected executeCommandConfiguration logging OVERRIDE with logConfiguration), $.cluster.settings[0] missing.
Cause 8 (ecs-container-instance-and-attribute-errors#teardown#14, DeleteCluster): $.cluster.registeredContainerInstancesCount: value_mismatch (expected 0, got 1).

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus (replay_wire_corpus).
2. Fixture ecs-cluster-capacity-provider-lifecycle: steps task#5 (settings update not visible), task#9 (missing UpdateInProgressException), task#10 (attachments never complete).
3. By hand: CreateCluster; UpdateClusterSettings{settings:[{name:containerInsights,value:enabled}]}; DescribeClusters{include:[SETTINGS]} -> value is still 'disabled'. Then PutClusterCapacityProviders and DescribeClusters{include:[ATTACHMENTS]} -> no attachmentsStatus; DeleteCluster during the update -> 200 instead of 400 UpdateInProgressException.
