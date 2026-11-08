Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-cluster-state-roles-and-capacity-operations
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-cluster-state-roles-and-capacity-operations rds-export-task-lifecycle rds-restore-pit-and-export-errors
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|AddRoleToDBCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|RemoveRoleFromDBCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|StartExportTask|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: rds|StartExportTask|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|StartExportTask|4xx->4xx|status
Issue-Commit: f1d789f71099019e66841b6c76416b94c6fc417d

# rds cluster-role and export refusals answer the wrong message and the wrong error class

> _Note: this issue was identified against the code base at commit `f1d789f71099019e66841b6c76416b94c6fc417d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddRoleToDBCluster

## Description
Three net-new root causes on RDS refusal paths: two of them re-word the DBClusterNotFoundFault sentence, and the third refuses in the wrong error class entirely because validation runs in the wrong order. A fixer must close ALL THREE before submitting.

Cause 1 of 3: AddRoleToDBCluster against an absent DB cluster re-words the not-found refusal.
AWS: Smithy: AddRoleToDBCluster errors include DBClusterNotFoundFault (errorKind client, httpError 404, awsQueryError code DBClusterNotFoundFault / 404), raised when the DBClusterIdentifier in the request resolves to no cluster. The sentence itself is recorded behavior only: the recording shows HTTP 404, Error.Code DBClusterNotFoundFault and Error.Message "DBCluster not found: <DBClusterIdentifier>" — the fixed text is "DBCluster not found: " and the only variable part is the identifier copied verbatim from the request's DBClusterIdentifier member.
Emulator: same 404, but Error.Message is the emulator's own prose "The source cluster could not be found or cannot be accessed: <DBClusterIdentifier>" — the sentence AWS uses for an inaccessible *source* cluster on a copy/replicate path, not the plain not-found sentence for this operation.
Why: a client that keys on the refusal text (log scraping, test assertions, or the common pattern of distinguishing "cluster absent" from "cluster present but not accessible/authorized") mis-classifies a plain missing-cluster condition as an access or cross-region source problem and retries or escalates instead of creating the cluster.
Fix: the not-found refusal raised by the AddRoleToDBCluster handler's cluster lookup (its validation step), which is reaching for the shared source-cluster-unavailable message instead of the DBClusterNotFound message for this operation.
Traces: rds-db-cluster-state-roles-and-capacity-operations#task#2.

Cause 2 of 3: RemoveRoleFromDBCluster against an absent DB cluster re-words the same not-found refusal.
AWS: Smithy: RemoveRoleFromDBCluster errors include DBClusterNotFoundFault (client, httpError 404, awsQueryError DBClusterNotFoundFault / 404). Recorded behavior: HTTP 404, Error.Code DBClusterNotFoundFault, Error.Message "DBCluster not found: <DBClusterIdentifier>" — fixed text "DBCluster not found: " plus the request's DBClusterIdentifier.
Emulator: 404 with "The source cluster could not be found or cannot be accessed: <DBClusterIdentifier>".
Why: same client mis-classification as cause 1, on the role-removal path; a teardown that tolerates "cluster already gone" by matching the not-found sentence will not recognise the refusal and will fail the teardown.
Fix: the RemoveRoleFromDBCluster handler's cluster-lookup refusal (its validation step); both this and cause 1 come from the same shared helper message, but each operation's refusal must be verified.
Traces: rds-db-cluster-state-roles-and-capacity-operations#task#3.

Cause 3 of 3: StartExportTask validates the snapshot before the KMS key, so an inaccessible KMS key is reported as a missing snapshot — wrong status AND wrong error code.
AWS: Smithy: StartExportTask requires both SourceArn and KmsKeyId and models both DBSnapshotNotFoundFault (client, httpError 404, awsQueryError DBSnapshotNotFound / 404) and KMSKeyNotAccessibleFault (client, httpError 400, awsQueryError KMSKeyNotAccessibleFault / 400). The precedence is recorded behavior only: with a request that names BOTH a non-existent snapshot ARN and a non-existent KMS key, AWS answers HTTP 400, Error.Code KMSKeyNotAccessibleFault, Error.Message "The KMS key <KmsKeyId> doesn't exist or is disabled." — the fixed text is "The KMS key " ... " doesn't exist or is disabled." and the variable part is the key ARN/id taken verbatim from the request's KmsKeyId member. The KMS key is therefore checked before the export source is resolved.
Emulator: HTTP 404, Error.Code DBSnapshotNotFound, Error.Message "DBSnapshot not found: <SourceArn>" — it resolves the source snapshot first and never reaches the KMS check, so the KMS failure is never reportable.
Why: the caller gets a 404 naming the snapshot when the real, and only actionable, fault is the KMS key its export would be encrypted with. Retry policy differs too (a 404 not-found is commonly treated as fatal-but-expected while a 400 KMSKeyNotAccessibleFault points at key policy/grants), so an export pipeline debugs the wrong resource and never fixes the key.
Fix: the input-validation ordering in the StartExportTask handler — validate KmsKeyId accessibility before resolving SourceArn, and surface KMSKeyNotAccessibleFault with its 400 status.
Traces: rds-export-task-lifecycle#task#3, rds-restore-pit-and-export-errors#task#2.

Related open issues that do NOT cover these: 20261106T224204Z-rds-not-found-and-state-guard-refusal-messages.md and 20261104T053605Z-rds-10-net-new-causes-refusal-messages-are.md cover other RDS refusal sentences, and the replay reports these three steps as unowned. The stub issues 20261104T053605Z-rds-addroletodbcluster-stub.md and 20261104T053605Z-rds-startexporttask-stub.md are stale for this purpose: both operations now answer on the wire (404/404 above), so no unimplemented-stub issue blocks these clusters.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — fixture rds-db-cluster-state-roles-and-capacity-operations, step task#2, AddRoleToDBCluster, status 404 -> 404:
  $.ErrorResponse.Error.Message: value_mismatch
    expected: 'DBCluster not found: sim2real-probe-rds-rdsclusterrolemissing-68cbf8da'
    actual:   'The source cluster could not be found or cannot be accessed: sim2real-probe-rds-rdsclusterrolemissing-68cbf8da'
  (the identifier is the request's DBClusterIdentifier; only the sentence around it is a requirement)

Cause 2 — fixture rds-db-cluster-state-roles-and-capacity-operations, step task#3, RemoveRoleFromDBCluster, status 404 -> 404:
  $.ErrorResponse.Error.Message: value_mismatch
    expected: 'DBCluster not found: <DBClusterIdentifier>'
    actual:   'The source cluster could not be found or cannot be accessed: <DBClusterIdentifier>'

Cause 3 — fixture rds-export-task-lifecycle, step task#3, StartExportTask, status expected 400, got 404 (also seen at rds-restore-pit-and-export-errors#task#2):
  status: expected 400, got 404
  $.ErrorResponse.Error.Code: expected 'KMSKeyNotAccessibleFault', actual 'DBSnapshotNotFound'
  $.ErrorResponse.Error.Message: expected "The KMS key <KmsKeyId> doesn't exist or is disabled.", actual 'DBSnapshot not found: <SourceArn>'

Smithy (services/service-com-amazonaws-rds-amazonrdsv19): AddRoleToDBCluster/RemoveRoleFromDBCluster -> DBClusterNotFoundFault httpError 404; StartExportTask -> KMSKeyNotAccessibleFault httpError 400 (awsQueryError KMSKeyNotAccessibleFault) and DBSnapshotNotFoundFault httpError 404 (awsQueryError DBSnapshotNotFound).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture rds-db-cluster-state-roles-and-capacity-operations step task#2 — AddRoleToDBCluster with a DBClusterIdentifier that does not exist and a well-formed RoleArn. Expect 404 / DBClusterNotFoundFault / "DBCluster not found: <identifier>".
3. Cause 2: replay the same fixture step task#3 — RemoveRoleFromDBCluster with the same absent DBClusterIdentifier. Expect the same 404 sentence.
4. Cause 3: replay fixture rds-export-task-lifecycle step task#3 (or rds-restore-pit-and-export-errors step task#2) — StartExportTask whose SourceArn names a non-existent DB snapshot AND whose KmsKeyId names a non-existent key. Expect 400 / KMSKeyNotAccessibleFault / "The KMS key <KmsKeyId> doesn't exist or is disabled.", not 404 / DBSnapshotNotFound.
