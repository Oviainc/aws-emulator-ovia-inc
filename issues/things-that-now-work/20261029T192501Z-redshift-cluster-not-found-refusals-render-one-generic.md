Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-cluster-config-op-preconditions
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-cluster-config-op-preconditions redshift-cluster-lifecycle-op-preconditions redshift-snapshot-op-preconditions-and-batch-results redshift-usage-limit-endpoint-and-scheduled-action-preconditions redshift-namespace-registration-preconditions
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|ModifyClusterIamRoles|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|EnableSnapshotCopy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DisableSnapshotCopy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|ModifySnapshotCopyRetentionPeriod|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|ModifyCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|CreateClusterSnapshot|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|CreateUsageLimit|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|RegisterNamespace|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DeregisterNamespace|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# Redshift cluster-not-found refusals render one generic message template instead of each operation's AWS wording

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyClusterIamRoles

## Description
9 root causes, all the same defect: every cluster-scoped Redshift operation that must refuse an unknown ClusterIdentifier formats its ClusterNotFoundFault message from ONE emulator-local template, while AWS uses a per-operation fixed message. Status (404) and awsQuery error code (ClusterNotFound) are correct everywhere; only the Message element diverges. Smithy: ClusterNotFoundFault is an @error("client") with httpError 404 and awsQueryError code ClusterNotFound, bound to all nine operations below.

Two distinct AWS templates are in play, and the emulator gets both wrong by using a single one:
  (a) "Cluster not found: <ClusterIdentifier>" — fixed text "Cluster not found: ", variable part is the ClusterIdentifier taken from the request.
  (b) "Cluster <ClusterIdentifier> can't be found." — fixed text "Cluster " + " can't be found.", variable part is the request's ClusterIdentifier.
The emulator emits "Cluster <ClusterIdentifier> not found." for the (a) group and "Cluster not found: <ClusterIdentifier>" for the (b) group — i.e. the two templates are swapped/collapsed.

Cause 1 of 9: ModifyClusterIamRoles, unknown cluster. AWS: 404 ClusterNotFound, Message "Cluster not found: <ClusterIdentifier>". Emulator: 404 ClusterNotFound, Message "Cluster <ClusterIdentifier> not found." Why: a client matching on the refusal text (or a log/alarm regex) does not recognize the refusal. Fix: the ModifyClusterIamRoles handler's cluster-lookup refusal path must use the operation's AWS message template, not the shared one. Traces: redshift-cluster-config-op-preconditions#task#1.

Cause 2 of 9: EnableSnapshotCopy, unknown cluster. AWS: 404 ClusterNotFound, "Cluster not found: <ClusterIdentifier>". Emulator: "Cluster <ClusterIdentifier> not found." Why/Fix: as cause 1, in the EnableSnapshotCopy handler. Traces: redshift-cluster-config-op-preconditions#task#6.

Cause 3 of 9: DisableSnapshotCopy, unknown cluster. AWS: 404 ClusterNotFound, "Cluster not found: <ClusterIdentifier>". Emulator: "Cluster <ClusterIdentifier> not found." Fix: DisableSnapshotCopy handler refusal path. Traces: redshift-cluster-config-op-preconditions#task#7.

Cause 4 of 9: ModifySnapshotCopyRetentionPeriod, unknown cluster. AWS: 404 ClusterNotFound, "Cluster not found: <ClusterIdentifier>". Emulator: "Cluster <ClusterIdentifier> not found." Fix: ModifySnapshotCopyRetentionPeriod handler refusal path. Traces: redshift-cluster-config-op-preconditions#task#8.

Cause 5 of 9: ModifyCluster, unknown cluster. AWS: 404 ClusterNotFound, "Cluster not found: <ClusterIdentifier>". Emulator: "Cluster <ClusterIdentifier> not found." Fix: ModifyCluster handler refusal path. Traces: redshift-cluster-lifecycle-op-preconditions#task#1.

Cause 6 of 9: CreateClusterSnapshot, unknown cluster. AWS: 404 ClusterNotFound, "Cluster not found: <ClusterIdentifier>". Emulator: "Cluster <ClusterIdentifier> not found." Fix: CreateClusterSnapshot handler refusal path. Traces: redshift-snapshot-op-preconditions-and-batch-results#task#1.

Cause 7 of 9: CreateUsageLimit, unknown cluster. AWS: 404 ClusterNotFound, "Cluster not found: <ClusterIdentifier>". Emulator: "Cluster <ClusterIdentifier> not found." Fix: CreateUsageLimit handler refusal path. Traces: redshift-usage-limit-endpoint-and-scheduled-action-preconditions#task#1.

Cause 8 of 9: RegisterNamespace, unknown cluster. AWS uses the OTHER template here: 404 ClusterNotFound, Message "Cluster <ClusterIdentifier> can't be found." Emulator: "Cluster not found: <ClusterIdentifier>". Fix: RegisterNamespace handler refusal path must use the "can't be found." template. Traces: redshift-namespace-registration-preconditions#task#1.

Cause 9 of 9: DeregisterNamespace, unknown cluster. AWS: 404 ClusterNotFound, "Cluster <ClusterIdentifier> can't be found." Emulator: "Cluster not found: <ClusterIdentifier>". Fix: DeregisterNamespace handler refusal path. Traces: redshift-namespace-registration-preconditions#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1, verdict=mismatch on all nine clusters; expected_status == actual_status == 404 and Error.Code matches, only $.ErrorResponse.Error.Message diverges.

Representative (ModifyClusterIamRoles, redshift-cluster-config-op-preconditions#task#1):
  $.ErrorResponse.Error.Message: expected 'Cluster not found: sim2real-probe-cluster-missing', got 'Cluster sim2real-probe-cluster-missing not found.'
EnableSnapshotCopy (#task#6), DisableSnapshotCopy (#task#7), ModifySnapshotCopyRetentionPeriod (#task#8), ModifyCluster (redshift-cluster-lifecycle-op-preconditions#task#1), CreateClusterSnapshot (redshift-snapshot-op-preconditions-and-batch-results#task#1), CreateUsageLimit (redshift-usage-limit-endpoint-and-scheduled-action-preconditions#task#1): identical expected/got pair.
RegisterNamespace (redshift-namespace-registration-preconditions#task#1) and DeregisterNamespace (#task#2):
  expected "Cluster sim2real-probe-cluster-missing can't be found.", got 'Cluster not found: sim2real-probe-cluster-missing'

The identifier shown is the one supplied in the request; the surrounding text is the part that must match AWS exactly.

## Steps to Reproduce
1. Replay fixture redshift-cluster-config-op-preconditions against the emulator and inspect step task#1 (ModifyClusterIamRoles with a ClusterIdentifier that does not exist).
2. Compare $.ErrorResponse.Error.Message with the recorded response.
3. Repeat for steps task#6, task#7, task#8 of the same fixture and for redshift-cluster-lifecycle-op-preconditions#task#1, redshift-snapshot-op-preconditions-and-batch-results#task#1, redshift-usage-limit-endpoint-and-scheduled-action-preconditions#task#1, redshift-namespace-registration-preconditions#task#1 and #task#2.
