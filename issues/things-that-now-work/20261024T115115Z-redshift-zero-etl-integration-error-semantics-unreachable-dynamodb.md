Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.integration-error-semantics redshift.snapshot-restore-preserves-table-data
Issue-Key: redshift|CreateTable|NotImplemented
Issue-Key: redshift.snapshot-restore-preserves-table-data|_wait_until#30|Timeout|time.sleep(interval_s)
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Redshift zero-ETL integration error semantics unreachable (DynamoDB CreateTable with SSESpecification answers NotImplemented), and snapshot-restore-with-data never completes (2 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIntegration

## Description
Two net-new root causes found replaying the redshift behavioral checks. Both are in the redshift suite but one of them is fixed in the DynamoDB CreateTable handler — the issue's service token is taken from the failing check id (`redshift.*`), the fix location is named per cause. A fixer must close BOTH causes before submitting.

=== Cause 1 of 2: CreateIntegration's source/target validation can never be exercised because DynamoDB CreateTable refuses SSESpecification ===
Failing check: redshift.integration-error-semantics

AWS: DynamoDB CreateTable accepts an SSESpecification ({Enabled:true, SSEType:"KMS", KMSMasterKeyId:<cmk>}) and returns 200 with a TableDescription; the table then becomes ACTIVE and can be used as a zero-ETL source. Redshift then validates CreateIntegration's SourceArn and TargetArn existence synchronously and up front. Smithy (redshift): CreateIntegration input SourceArn @required @length 1..255 @pattern ^arn:aws[a-z\-]*:(s3|dynamodb):.*:.*:[a-zA-Z0-9._\-/]+$, TargetArn @required @length 20..2048 @pattern ^arn:aws[a-z\-]*:redshift(-serverless)?:...:(namespace/|namespace:)...$; modeled errors IntegrationSourceNotFoundFault (httpError 404, awsQueryError IntegrationSourceNotFoundFault), IntegrationTargetNotFoundFault (404), IntegrationAlreadyExistsFault (400), IntegrationConflictOperationFault (400), IntegrationQuotaExceededFault (400).

Emulator: the check aborts during setup, before any Redshift call: botocore ClientError "An error occurred (NotImplemented) when calling the CreateTable operation: operation CreateTable with SSESpecification is scaffolded but not implemented in this emulator". So CreateTable is implemented for the plain case but refuses the request the moment SSESpecification is present. (The separately-filed DynamoDB UpdateContinuousBackups stub issue is a DIFFERENT operation and does not cover this; the suite did not mark this failure as already filed.)

Why: a client cannot create a KMS-encrypted DynamoDB table at all — any CreateTable carrying SSESpecification fails outright instead of returning a table — and, downstream, no zero-ETL source can exist, so every CreateIntegration not-found/conflict/quota refusal class is unreachable and a caller cannot tell a validating emulator from a permissive one.

Fix: the DynamoDB CreateTable handler's SSESpecification path (services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/create_table.rs) must accept and record server-side encryption instead of answering NotImplemented; once it does, the redshift CreateIntegration handler's SourceArn/TargetArn existence validation must raise the modeled IntegrationSourceNotFoundFault / IntegrationTargetNotFoundFault.

Traces: redshift.integration-error-semantics

=== Cause 2 of 2: the snapshot-contains-data lifecycle never completes — the check hangs to the 60 s harness cap ===
Failing check: redshift.snapshot-restore-preserves-table-data

AWS: CreateCluster followed by DescribeClusters moves ClusterStatus through creating -> available; CreateClusterSnapshot returns a Snapshot whose SnapshotType is "manual" and whose Status reaches "available" on DescribeClusterSnapshots; RestoreFromClusterSnapshot then yields a new cluster whose SQL engine returns exactly the rows captured in the snapshot (and not rows written to the source after it). Smithy: CreateClusterSnapshot -> CreateClusterSnapshotResult{Snapshot}, errors ClusterNotFoundFault (404, ClusterNotFound), ClusterSnapshotAlreadyExistsFault (400), InvalidClusterStateFault (400); RestoreFromClusterSnapshot -> RestoreFromClusterSnapshotResult{Cluster}, errors ClusterSnapshotNotFoundFault (404, ClusterSnapshotNotFound), InvalidRestoreFault (406, InvalidRestore).

Emulator: the check produces no verdict at all — "Check timed out after 60.0s". It makes no observable progress through the cluster-available / statement-terminal / snapshot-available sequence within the budget, so the restore half is never reached. Recorded behavior only for the stall point; the replay output carries no frame. Hypothesis: one of the bounded polls (DescribeClusters ClusterStatus -> available, redshift-data DescribeStatement -> a terminal status, or DescribeClusterSnapshots Status -> available) never satisfies its predicate, so the check spins until the harness kills it.

Why: a client that snapshots a cluster and restores it gets no completion signal in bounded time; the single invariant a snapshot exists for — that it carries the cluster's table data — cannot be observed, and a metadata-only emulator that reports an 'available' snapshot plus an empty restored database is indistinguishable from a correct one.

Fix: the redshift cluster/snapshot state machine and its restore path — the CreateCluster/DescribeClusters status transition, CreateClusterSnapshot/DescribeClusterSnapshots status transition, and RestoreFromClusterSnapshot's materialization of the snapshot's data into the new cluster — must each converge to their terminal state promptly, and the restored cluster's SQL engine must serve the snapshot's rows.

Traces: redshift.snapshot-restore-preserves-table-data

## Evidence
Cause 1 — expected: dynamodb.create_table(..., SSESpecification={'Enabled': True, 'SSEType': 'KMS', 'KMSMasterKeyId': <cmk>}) returns HTTP 200 with a TableDescription, the table reaches TableStatus ACTIVE, and redshift.create_integration then refuses a wholly nonexistent SourceArn/TargetArn synchronously with IntegrationSourceNotFoundFault / IntegrationTargetNotFoundFault (HTTP 404). Actual: ClientError (NotImplemented) when calling the CreateTable operation: 'operation CreateTable with SSESpecification is scaffolded but not implemented in this emulator' — the check dies in setup and no CreateIntegration refusal class is ever observed.

Cause 2 — expected: the create-cluster -> seed rows -> CreateClusterSnapshot (SnapshotType 'manual') -> snapshot Status 'available' -> RestoreFromClusterSnapshot -> SELECT returns exactly the three snapshotted rows in id order, each wait returning the instant its condition holds. Actual: 'Check timed out after 60.0s' — no phase of that sequence completes, so no verdict about snapshot contents is produced at all.

## Steps to Reproduce
Cause 1: call dynamodb CreateTable with KeySchema/AttributeDefinitions, BillingMode=PAY_PER_REQUEST and SSESpecification={Enabled:true, SSEType:KMS, KMSMasterKeyId:<customer-managed key>}; observe NotImplemented instead of a TableDescription. Then (once that is fixed) call redshift CreateIntegration with a nonexistent dynamodb SourceArn and a nonexistent redshift namespace TargetArn and observe whether IntegrationSourceNotFoundFault / IntegrationTargetNotFoundFault (404) come back synchronously. Replay: redshift.integration-error-semantics.

Cause 2: redshift CreateCluster (single-node, Encrypted, DBName=devdb); poll DescribeClusters for ClusterStatus=available; run CREATE TABLE/INSERT over redshift-data; CreateClusterSnapshot; poll DescribeClusterSnapshots for Status=available; insert one more row into the source; RestoreFromClusterSnapshot into a new cluster; SELECT id,val ORDER BY id on the restored cluster and compare to the three pre-snapshot rows. Replay: redshift.snapshot-restore-preserves-table-data.
