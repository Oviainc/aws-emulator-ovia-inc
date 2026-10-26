Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.snapshot-share-and-copy
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift.snapshot-share-and-copy|run#21|AssertionError|assert copy_code == "InvalidParameterValue", (
Issue-Commit: b6854e7dd33056612488505988bd6062274116ad

# Redshift CopyClusterSnapshot answers 501 NotImplemented, so copying a manual snapshot is never refused with InvalidParameterValue (1 cause)

> _Note: this issue was identified against the code base at commit `b6854e7dd33056612488505988bd6062274116ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CopyClusterSnapshot

## Description
One root cause, in the redshift service.

Cause 1 of 1: CopyClusterSnapshot is an unimplemented scaffold stub, so the manual-vs-automated source rule it exists to enforce can never be observed, and no snapshot can ever be copied.

AWS: CopyClusterSnapshot copies an AUTOMATED cluster snapshot into a new manual snapshot; the Smithy documentation for the operation states "The source must be an automated snapshot and it must be in the available state." A request whose SourceSnapshotIdentifier names a MANUAL snapshot is therefore refused, and the recorded refusal is HTTP 400 with error code `InvalidParameterValue` and message "Only automated snapshots can be copied." — that message is entirely fixed text for this error class, with no variable part taken from the request (recorded behavior for the code and message; the model declares no dedicated shape for this rule). Smithy (com.amazonaws.redshift#CopyClusterSnapshot): input SourceSnapshotIdentifier (required), SourceSnapshotClusterIdentifier, TargetSnapshotIdentifier (required), ManualSnapshotRetentionPeriod; output Snapshot; modeled errors ClusterNotFoundFault (httpError 404, awsQueryError code `ClusterNotFound`), ClusterSnapshotAlreadyExistsFault (400, `ClusterSnapshotAlreadyExists`), ClusterSnapshotNotFoundFault (404, `ClusterSnapshotNotFound`), ClusterSnapshotQuotaExceededFault (400, `ClusterSnapshotQuotaExceeded`), InvalidClusterSnapshotStateFault (400, `InvalidClusterSnapshotState`), InvalidRetentionPeriodFault (400, `InvalidRetentionPeriodFault`). A refused copy must also leave no target snapshot behind: DescribeClusterSnapshots for the TargetSnapshotIdentifier must not return it.

Emulator: every CopyClusterSnapshot call answers HTTP 501 with error code `NotImplemented` and message "operation CopyClusterSnapshot is scaffolded but not implemented in this emulator". The handler at services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/copy_cluster_snapshot.rs still carries the SCAFFOLDED marker and returns `Scaffolded::not_implemented("CopyClusterSnapshot")` unconditionally, so neither the happy path (copying an available automated snapshot into a manual one) nor any of the six modeled refusals nor the manual-source `InvalidParameterValue` refusal exists.

Note: this cause is not covered by any open issue. There is no "Implement redshift CopyClusterSnapshot" stub issue, and the related open issues name different operations (20261024T024248Z-redshift-createcluster-skips-request-validation-and-publishes-an.md covers CreateSnapshotCopyGrant, 20261025T121048Z-redshift-snapshot-restore-access-revoke-and-contended-transaction.md covers Revoke/AuthorizeSnapshotAccess, 20261025T012958Z-redshift-deleteclustersnapshot-stub.md covers DeleteClusterSnapshot). Because the 501 is raised mid-check, the remainder of redshift.snapshot-share-and-copy — the BatchDeleteClusterSnapshots per-item error contract (Resources listing only the deleted snapshot, and one Errors entry carrying SnapshotIdentifier / FailureCode `ClusterSnapshotNotFoundFault` / FailureReason "Snapshot does not exist for the customer.") — is never reached and remains ungraded; it is not claimed as a separate cause here, but implementing CopyClusterSnapshot will expose it.

Why: a client that uses CopyClusterSnapshot to pin an expiring automated snapshot as a manual one gets an unmodelled 501 instead of either a Snapshot or one of the operation's modeled errors, so retention/backup tooling has no working path and no decidable failure branch; and a client's negative test that asserts a manual source is rejected with InvalidParameterValue sees `NotImplemented` instead, so the manual-vs-automated rule looks unenforced rather than enforced.

Fix: implement the handler in services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/copy_cluster_snapshot.rs (resolve the source snapshot in the redshift snapshot registry in that crate's state, enforce source-is-automated and source-is-available, enforce target uniqueness and the retention-period range, then mint the manual snapshot) and delete that file's SCAFFOLDED marker line.

Traces: redshift.snapshot-share-and-copy.

## Evidence
Cause 1 — redshift.snapshot-share-and-copy (verdict: mismatch):
  File "check_redshift_snapshot_share_and_copy.py", line 349, in run
    assert copy_code == "InvalidParameterValue", (
  AssertionError: copying a manual snapshot must be rejected, got NotImplemented

The check reaches this point having already created a single-node encrypted cluster, taken a MANUAL snapshot of it and round-tripped AuthorizeSnapshotAccess/RevokeSnapshotAccess successfully, then calls:
  redshift.copy_cluster_snapshot(SourceSnapshotIdentifier=<the manual snapshot>, TargetSnapshotIdentifier=<fresh id>)

Expected (AWS): HTTP 400, Error.Code `InvalidParameterValue`, Error.Message "Only automated snapshots can be copied." (fixed text; no request- or state-derived substitutions), and DescribeClusterSnapshots for the target identifier returns no snapshot.
Observed (emulator): HTTP 501, Error.Code `NotImplemented`, Error.Message "operation CopyClusterSnapshot is scaffolded but not implemented in this emulator".

Source of the 501 — services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/copy_cluster_snapshot.rs:4 (`//! SCAFFOLDED, NOT IMPLEMENTED:` marker) and :187 (`Err(crate::scaffolded::Scaffolded::not_implemented("CopyClusterSnapshot"))`).

(Cluster, snapshot and target identifiers are minted per run; they are examples, not part of the requirement.)

## Steps to Reproduce
1. Boot the emulator and point a redshift client (plus ec2 for the VPC/subnet) at it.
2. Create a VPC, a subnet and a cluster subnet group, then CreateCluster a single-node cluster and wait for ClusterStatus 'available'.
3. CreateClusterSnapshot against that cluster (this yields a MANUAL snapshot) and wait for Status 'available'.
4. Call CopyClusterSnapshot with SourceSnapshotIdentifier = that manual snapshot and a fresh TargetSnapshotIdentifier.
   Expect: HTTP 400, code InvalidParameterValue, message "Only automated snapshots can be copied."
   Observe: HTTP 501, code NotImplemented, message "operation CopyClusterSnapshot is scaffolded but not implemented in this emulator".
5. Call DescribeClusterSnapshots with the target identifier and confirm the refused copy materialised nothing (expect ClusterSnapshotNotFound).
6. Re-replay redshift.snapshot-share-and-copy to confirm the fix; it then continues into the BatchDeleteClusterSnapshots per-item error assertions, which this 501 currently prevents from ever running.
