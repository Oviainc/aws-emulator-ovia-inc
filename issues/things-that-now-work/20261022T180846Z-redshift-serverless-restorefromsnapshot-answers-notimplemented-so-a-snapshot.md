Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift-data__redshift-serverless.snapshot-restores-into-new-namespace-with-data
Issue-Key: redshift-data__redshift-serverless|RestoreFromSnapshot|NotImplemented
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Redshift Serverless RestoreFromSnapshot answers NotImplemented, so a snapshot can never be restored into a new namespace

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RestoreFromSnapshot

## Description
Grounded in the behavioral check `redshift-data__redshift-serverless.snapshot-restores-into-new-namespace-with-data` (a cross-service check; its divergence is wholly in redshift-serverless, which is why this issue is filed under that service token). This issue carries ONE cause.

=== Cause 1 of 1: RestoreFromSnapshot is an unimplemented scaffold stub ===
AWS: Smithy (com-amazonaws-redshiftserverless-redshiftserverless): RestoreFromSnapshot is a bound, @idempotent operation taking namespaceName (@required, 3..64, ^[a-z0-9-]+$), workgroupName (@required, 3..64, ^[a-z0-9-]+$), snapshotName, snapshotArn, ownerAccount, manageAdminPassword, adminPasswordSecretKmsKeyId and maintainIntegration, returning {snapshotName, ownerAccount, namespace: Namespace}. Its modeled errors are ValidationException (400), ResourceNotFoundException (404), ConflictException (409), ServiceQuotaExceededException (402) and InternalServerException (500); `NotImplemented` is not among them. On AWS the call restores the snapshot's data into the named namespace, and the tables written before the snapshot are then readable through the Data API against that namespace's workgroup.
Emulator: the call fails with error code `NotImplemented`; recorded message: "operation RestoreFromSnapshot is scaffolded but not implemented in this emulator". The check errors there, so nothing after the restore (the Data API read-back proving the restored rows are present) is exercised. The suite did not mark this check as already filed, and no open issue names RestoreFromSnapshot.
Why: snapshot/restore is the only backup-and-recovery path for Redshift Serverless. A client cannot restore a namespace at all, and gets an unmodeled error code its SDK error handling has no branch for instead of a modeled 2xx/4xx.
Fix: implement the redshift-serverless RestoreFromSnapshot operation handler (services/service-com-amazonaws-redshiftserverless-redshiftserverless/src/ops/ — the restore_from_snapshot handler) over the snapshot and namespace state: resolve snapshotName/snapshotArn, materialise the snapshot's databases/tables into the target namespace, and return {snapshotName, ownerAccount, namespace} with the modeled 404/409/400 rejections for an absent snapshot, a namespace already in use and a malformed name.
Traces: redshift-data__redshift-serverless.snapshot-restores-into-new-namespace-with-data.

## Evidence
Suite: simparity-behavioral-aws-v1, redshift-data__redshift-serverless 0/1 (0%), 1 failed (verdict=error).

  expected: RestoreFromSnapshot -> HTTP 200 with {snapshotName, ownerAccount, namespace}, after which the Data API reads the pre-snapshot rows back from the restored namespace
  actual:   botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the RestoreFromSnapshot operation: operation RestoreFromSnapshot is scaffolded but not implemented in this emulator

Model corroboration (services/service-com-amazonaws-redshiftserverless-redshiftserverless/smithy-model/com-amazonaws-redshiftserverless-redshiftserverless.json): RestoreFromSnapshot input namespaceName/workgroupName @required; output members snapshotName, ownerAccount, namespace; errors ValidationException 400, ResourceNotFoundException 404, ConflictException 409, ServiceQuotaExceededException 402, InternalServerException 500; @idempotent.

## Steps to Reproduce
1. Clone the repo and boot the emulator; point a botocore redshift-serverless client at it.
2. create_namespace + create_workgroup, write a table through redshift-data, then create_snapshot of the namespace.
3. Call restore_from_snapshot(namespaceName='<new-ns>', workgroupName='<new-wg>', snapshotName='<snapshot>') -> expect HTTP 200 with the restored namespace; observe ClientError NotImplemented 'operation RestoreFromSnapshot is scaffolded but not implemented in this emulator'.
4. Re-replay redshift-data__redshift-serverless.snapshot-restores-into-new-namespace-with-data to confirm the fix (it also asserts the restored rows are readable through the Data API).
