Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-snapshot-op-preconditions-and-batch-results
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-snapshot-op-preconditions-and-batch-results redshift-resource-policy-and-reserved-node-exchange-preconditions
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|ModifyClusterSnapshot|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|CopyClusterSnapshot|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|AuthorizeSnapshotAccess|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|RevokeSnapshotAccess|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|RestoreFromClusterSnapshot|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|BatchDeleteClusterSnapshots|2xx->2xx|$.BatchDeleteClusterSnapshotsResponse.BatchDeleteClusterSnapshotsResult.Errors.SnapshotErrorMessage[*].SnapshotClusterIdentifier
Issue-Key: redshift|BatchModifyClusterSnapshots|2xx->2xx|$.BatchModifyClusterSnapshotsResponse.BatchModifyClusterSnapshotsResult.Errors.SnapshotErrorMessage[*].FailureCode
Issue-Key: redshift|BatchModifyClusterSnapshots|2xx->2xx|$.BatchModifyClusterSnapshotsResponse.BatchModifyClusterSnapshotsResult.Errors.SnapshotErrorMessage[*].FailureReason
Issue-Key: redshift|BatchModifyClusterSnapshots|2xx->2xx|$.BatchModifyClusterSnapshotsResponse.BatchModifyClusterSnapshotsResult.Errors.SnapshotErrorMessage[*].SnapshotIdentifier
Issue-Key: redshift|GetReservedNodeExchangeConfigurationOptions|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# Redshift snapshot refusal messages and batch snapshot error payloads diverge from AWS

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyClusterSnapshot

## Description
8 root causes on the cluster-snapshot surface: six message-wording divergences on snapshot-not-found refusals, and two defects in the per-item error payloads of the batch snapshot operations.

Cause 1 of 8: ModifyClusterSnapshot for an unknown snapshot. Smithy: ClusterSnapshotNotFoundFault @error("client"), httpError 404, awsQueryError ClusterSnapshotNotFound. AWS: 404, Message "ClusterSnapshot not found: <SnapshotIdentifier>" — fixed text "ClusterSnapshot not found: " (the CamelCase shape name), variable part is the request's SnapshotIdentifier, no trailing period. Emulator: "Cluster snapshot <SnapshotIdentifier> not found." Why: clients keying on AWS's refusal text mis-classify the refusal. Fix: ModifyClusterSnapshot handler's snapshot-lookup refusal path. Traces: redshift-snapshot-op-preconditions-and-batch-results#task#3.

Cause 2 of 8: CopyClusterSnapshot with an unknown source snapshot. AWS: 404 ClusterSnapshotNotFound, "ClusterSnapshot not found: <SourceSnapshotIdentifier>". Emulator: "Cluster snapshot <id> not found." Fix: CopyClusterSnapshot handler refusal path. Traces: redshift-snapshot-op-preconditions-and-batch-results#task#4.

Cause 3 of 8: AuthorizeSnapshotAccess for an unknown snapshot. AWS uses a DIFFERENT template here: 404 ClusterSnapshotNotFound, "Snapshot <SnapshotIdentifier> not found." — fixed text "Snapshot " + " not found.", identifier embedded, trailing period. Emulator: "Cluster snapshot <id> not found." Fix: AuthorizeSnapshotAccess handler refusal path. Traces: redshift-snapshot-op-preconditions-and-batch-results#task#5.

Cause 4 of 8: RevokeSnapshotAccess for an unknown snapshot. AWS: 404 ClusterSnapshotNotFound, "Snapshot <SnapshotIdentifier> not found." Emulator: "Cluster snapshot <id> not found." Fix: RevokeSnapshotAccess handler refusal path. Traces: redshift-snapshot-op-preconditions-and-batch-results#task#6.

Cause 5 of 8: RestoreFromClusterSnapshot with an unknown snapshot. AWS uses a THIRD template: 404 ClusterSnapshotNotFound, "Snapshot not found: <SnapshotIdentifier>" — colon form, no period. Emulator: "Cluster snapshot <id> not found." Fix: RestoreFromClusterSnapshot handler refusal path. Traces: redshift-snapshot-op-preconditions-and-batch-results#task#7.

Cause 6 of 8: GetReservedNodeExchangeConfigurationOptions with an unknown SnapshotIdentifier. AWS: 404 ClusterSnapshotNotFound, "Snapshot not found: <SnapshotIdentifier>". Emulator: "The snapshot <SnapshotIdentifier> was not found." — a fourth, entirely emulator-authored sentence. Fix: GetReservedNodeExchangeConfigurationOptions handler's snapshot-lookup refusal path. Traces: redshift-resource-policy-and-reserved-node-exchange-preconditions#task#5.

Cause 7 of 8: BatchDeleteClusterSnapshots per-item error entries omit SnapshotClusterIdentifier. Smithy: SnapshotErrorMessage has members SnapshotIdentifier, SnapshotClusterIdentifier, FailureCode and FailureReason; AWS serializes SnapshotClusterIdentifier as an EMPTY element for an identifier-only delete request (the member is present with no value). The emulator omits the element entirely, so an awsQuery/XML-shaped consumer that expects every modeled member to be present sees a different document, and a strict deserializer sees a shorter struct. Why: a client iterating the Errors list cannot rely on the member being present. Fix: the BatchDeleteClusterSnapshots serializer for SnapshotErrorMessage must emit SnapshotClusterIdentifier even when it has no value. Traces: redshift-snapshot-op-preconditions-and-batch-results#task#10.

Cause 8 of 8: BatchModifyClusterSnapshots per-item error entries carry the wrong FailureCode and are emitted in the wrong order. AWS sets FailureCode to the fault SHAPE name "ClusterSnapshotNotFoundFault"; the emulator sets the awsQuery short code "ClusterSnapshotNotFound". AWS's FailureReason uses the "ClusterSnapshot not found: <SnapshotIdentifier>" template (cause 1); the emulator uses "Cluster snapshot <id> not found." In addition the Errors list is ordered differently from AWS's: the recording pairs error[0] with the second requested identifier and error[1] with the first, while the emulator emits them in request order, so SnapshotIdentifier, FailureCode and FailureReason all disagree position-by-position. Why: a client that indexes Errors positionally, or that switches on FailureCode, mis-attributes failures to the wrong snapshot and does not recognize the failure class. Fix: the BatchModifyClusterSnapshots handler must populate FailureCode with the fault shape name, reuse the ClusterSnapshotNotFound message template for FailureReason, and emit Errors in AWS's order. Traces: redshift-snapshot-op-preconditions-and-batch-results#task#11.

## Evidence
Replay of simparity-wire-aws-v1; all eight clusters verdict=mismatch.

ModifyClusterSnapshot (#task#3) and CopyClusterSnapshot (#task#4), 404 vs 404:
  expected 'ClusterSnapshot not found: sim2real-probe-snap-missing', got 'Cluster snapshot sim2real-probe-snap-missing not found.'
AuthorizeSnapshotAccess (#task#5) and RevokeSnapshotAccess (#task#6), 404 vs 404:
  expected 'Snapshot sim2real-probe-snap-missing not found.', got 'Cluster snapshot sim2real-probe-snap-missing not found.'
RestoreFromClusterSnapshot (#task#7), 404 vs 404:
  expected 'Snapshot not found: sim2real-probe-snap-missing', got 'Cluster snapshot sim2real-probe-snap-missing not found.'
GetReservedNodeExchangeConfigurationOptions (redshift-resource-policy-and-reserved-node-exchange-preconditions#task#5), 404 vs 404:
  expected 'Snapshot not found: sim2real-probe-snap-missing', got 'The snapshot sim2real-probe-snap-missing was not found.'
BatchDeleteClusterSnapshots (#task#10), 200 vs 200:
  $...Errors.SnapshotErrorMessage[0].SnapshotClusterIdentifier: missing (expected '', got None)
  $...Errors.SnapshotErrorMessage[1].SnapshotClusterIdentifier: missing (expected '', got None)
BatchModifyClusterSnapshots (#task#11), 200 vs 200:
  [0].FailureCode: expected 'ClusterSnapshotNotFoundFault', got 'ClusterSnapshotNotFound'
  [0].FailureReason: expected 'ClusterSnapshot not found: sim2real-probe-snap-missing-b', got 'Cluster snapshot sim2real-probe-snap-missing-a not found.'
  [0].SnapshotIdentifier: expected 'sim2real-probe-snap-missing-b', got 'sim2real-probe-snap-missing-a'
  [1].FailureCode: expected 'ClusterSnapshotNotFoundFault', got 'ClusterSnapshotNotFound'
  [1].FailureReason: expected 'ClusterSnapshot not found: sim2real-probe-snap-missing-a', got 'Cluster snapshot sim2real-probe-snap-missing-b not found.'
  [1].SnapshotIdentifier: expected 'sim2real-probe-snap-missing-a', got 'sim2real-probe-snap-missing-b'

Snapshot identifiers shown come from the request; the fixed wording, the FailureCode spelling and the presence of SnapshotClusterIdentifier are the requirements.

## Steps to Reproduce
1. Replay fixture redshift-snapshot-op-preconditions-and-batch-results.
2. Inspect steps task#3, task#4, task#5, task#6 and task#7 (each naming a snapshot identifier that does not exist) and compare $.ErrorResponse.Error.Message with the recordings.
3. Inspect step task#10 (BatchDeleteClusterSnapshots over two unknown snapshots) and check that each Errors/SnapshotErrorMessage entry carries a SnapshotClusterIdentifier element.
4. Inspect step task#11 (BatchModifyClusterSnapshots over two unknown snapshots) and compare FailureCode, FailureReason and SnapshotIdentifier per entry, including entry order.
5. Replay fixture redshift-resource-policy-and-reserved-node-exchange-preconditions step task#5 (GetReservedNodeExchangeConfigurationOptions with an unknown snapshot).
