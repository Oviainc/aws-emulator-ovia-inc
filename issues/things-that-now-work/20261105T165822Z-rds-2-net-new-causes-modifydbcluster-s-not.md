Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.delete-deletion-protected-cluster-without-disable-rejected rds.delete-without-final-snapshot-identifier-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.delete-deletion-protected-cluster-without-disable-rejected|run#19|AssertionError|assert dead_message == "DBCluster not found", (
Issue-Key: rds.delete-without-final-snapshot-identifier-rejected|run#13|AssertionError|assert message == _MSG_NO_FINAL_SNAPSHOT_ID, f"unexpected error message {message!r}"
Issue-Commit: 6c23f9e5c79169c881c9ebb1fb44ffab91a88dbf

# rds: 2 net-new causes — ModifyDBCluster's not-found message embeds the cluster id, and DeleteDBInstance's missing-FinalDBSnapshotIdentifier refusal is emulator prose

> _Note: this issue was identified against the code base at commit `6c23f9e5c79169c881c9ebb1fb44ffab91a88dbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyDBCluster

## Description
Two net-new root causes in RDS refusal wording. Both are message-text divergences on refusals whose HTTP status and error code the emulator already gets right, so each is a separate handler/validation site. Related open issues exist for RDS refusal wording (20261102T074545Z-rds-create-path-refusal-messages-use-emulator-wording.md, 20261103T043005Z-rds-7-net-new-root-causes-refusal-messages.md, 20261104T053605Z-rds-10-net-new-causes-refusal-messages-are.md) but none of them carries either replay key below; these two operations/messages are not covered there.

Cause 1 of 2: ModifyDBCluster on a DB cluster identifier that does not exist returns the right code but the wrong message — it interpolates the identifier into the sentence.
AWS: Smithy: ModifyDBCluster error DBClusterNotFoundFault, errorKind=client, httpError 404, awsQueryError code DBClusterNotFoundFault / httpResponseCode 404; shape doc "DBClusterIdentifier doesn't refer to an existing DB cluster." Recorded behavior: AWS answers HTTP 404, Code=DBClusterNotFoundFault, and the message is the FIXED sentence "DBCluster not found" — it has NO variable part at all; the requested identifier is deliberately not echoed.
Emulator: same code, but Message='DBCluster <DBClusterIdentifier> not found.' — i.e. the emulator builds the sentence from the request's DBClusterIdentifier and appends a period. The replay shows 'DBCluster sim2real-delprot-3b16f68a61cc not found.' where sim2real-delprot-3b16f68a61cc is the per-run identifier from the request (not part of the requirement); the defect is the template, not the value.
Why: a client that matches, logs or asserts on RDS's not-found text (and any test or runbook that compares the mutation path's message to the read path's) sees a different sentence for every identifier, and never sees AWS's fixed "DBCluster not found". The identifier leaking into the message also changes the message's cardinality, defeating log grouping/alerting on that error class.
Fix: the ModifyDBCluster handler's cluster-lookup/not-found branch (and whatever shared cluster-lookup helper mints DBClusterNotFoundFault for mutating operations) must emit the fixed message with no identifier interpolation. The read path's wording must not be re-derived per call site.
Traces: rds.delete-deletion-protected-cluster-without-disable-rejected (the check first exercises the deletion-protection refusal, which passes, then asserts the post-delete ModifyDBCluster not-found message).

Cause 2 of 2: DeleteDBInstance with neither FinalDBSnapshotIdentifier nor SkipFinalSnapshot is refused with the emulator's own sentence instead of RDS's.
AWS: Smithy: DeleteDBInstance input members SkipFinalSnapshot (Boolean) and FinalDBSnapshotIdentifier (String); the model itself does not state the combination rule, so this is recorded behavior only. Recorded behavior: the call is refused HTTP 400 with Code=InvalidParameterCombination and the FIXED message "FinalDBSnapshotIdentifier is required unless SkipFinalSnapshot is specified." — the whole sentence is fixed text naming the two member names; there are no request-derived values in it.
Emulator: refuses with the right code but Message='Please specify FinalDBSnapshotIdentifier or SkipFinalSnapshot' — different wording, different member phrasing, and no trailing period.
Why: callers that branch on, surface, or snapshot-test the RDS delete-without-snapshot refusal text get a sentence AWS never emits; SDK/CLI users copying the message into docs or error handling diverge from AWS, and any assertion on the documented sentence fails even though the operation was correctly refused.
Fix: the DeleteDBInstance handler's FinalDBSnapshotIdentifier/SkipFinalSnapshot combination validation must emit AWS's fixed sentence verbatim.
Traces: rds.delete-without-final-snapshot-identifier-rejected.

## Evidence
Cause 1 — rds.delete-deletion-protected-cluster-without-disable-rejected
  Call: ModifyDBCluster(DBClusterIdentifier=<id of a cluster just deleted>, ApplyImmediately=true)
  Expected: HTTP 404, Error.Code='DBClusterNotFoundFault', Error.Message='DBCluster not found' (fixed text, no identifier)
  Actual:   Error.Code='DBClusterNotFoundFault', Error.Message='DBCluster sim2real-delprot-3b16f68a61cc not found.'
  Assertion: AssertionError: unexpected ModifyDBCluster not-found message on the deleted cluster id: 'DBCluster sim2real-delprot-3b16f68a61cc not found.'
  (sim2real-delprot-3b16f68a61cc is this run's generated identifier — the divergence is that the identifier appears at all, plus the added '.')

Cause 2 — rds.delete-without-final-snapshot-identifier-rejected
  Call: DeleteDBInstance(DBInstanceIdentifier=<available instance>) with neither SkipFinalSnapshot nor FinalDBSnapshotIdentifier
  Expected: HTTP 400, Error.Code='InvalidParameterCombination', Error.Message='FinalDBSnapshotIdentifier is required unless SkipFinalSnapshot is specified.'
  Actual:   Error.Message='Please specify FinalDBSnapshotIdentifier or SkipFinalSnapshot'
  Assertion: AssertionError: unexpected error message 'Please specify FinalDBSnapshotIdentifier or SkipFinalSnapshot'

## Steps to Reproduce
Cause 1:
1. CreateDBSubnetGroup over two AZs; CreateDBCluster(Engine='aurora-postgresql', DeletionProtection=true) and wait for Status='available'.
2. ModifyDBCluster(DeletionProtection=false, ApplyImmediately=true), then DeleteDBCluster(SkipFinalSnapshot=true) and wait until DescribeDBClusters reports it gone.
3. Call ModifyDBCluster(DBClusterIdentifier=<the deleted id>, ApplyImmediately=true) and read Error.Message. Observe the identifier interpolated into the sentence instead of the fixed 'DBCluster not found'.

Cause 2:
1. CreateDBInstance and wait for DBInstanceStatus='available'.
2. Call DeleteDBInstance(DBInstanceIdentifier=<that instance>) passing neither SkipFinalSnapshot nor FinalDBSnapshotIdentifier.
3. Read Error.Message: observe 'Please specify FinalDBSnapshotIdentifier or SkipFinalSnapshot' instead of 'FinalDBSnapshotIdentifier is required unless SkipFinalSnapshot is specified.'

Replay: run the behavioral suite checks rds.delete-deletion-protected-cluster-without-disable-rejected and rds.delete-without-final-snapshot-identifier-rejected.
