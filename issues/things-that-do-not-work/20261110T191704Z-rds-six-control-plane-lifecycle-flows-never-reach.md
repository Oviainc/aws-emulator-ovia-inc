Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.copy-db-snapshot-round-trip rds.copy-db-cluster-snapshot-round-trip rds.export-task-lifecycle rds.delete-automated-backup-by-arn rds.modify-apply-immediately-true-clears-a-prior-deferred-class-change rds.concurrent-failover-requests-single-winner
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.copy-db-snapshot-round-trip|_wait_until#10|Timeout|last = probe()
Issue-Key: rds.copy-db-cluster-snapshot-round-trip|_wait_until#13|Timeout|last = probe()
Issue-Key: rds.export-task-lifecycle|_wait_until#27|Timeout|return last
Issue-Key: rds.delete-automated-backup-by-arn|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: rds.modify-apply-immediately-true-clears-a-prior-deferred-class-change|_poll#13|Timeout|time.sleep(interval_s)
Issue-Key: rds.concurrent-failover-requests-single-winner|_wait_for_cluster_status#26|Timeout|return last
Issue-Commit: 122d2e23ed306dec374f1768a21e001304c75e9d

# rds: six control-plane lifecycle flows never reach their terminal status, so the checks that poll them are killed at the 60 s budget

> _Note: this issue was identified against the code base at commit `122d2e23ed306dec374f1768a21e001304c75e9d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBInstances

## Description
6 net-new root causes, all in the RDS control plane's status-convergence behaviour. Every one of the six checks below is killed with `Check timed out after 60.0s` — the emulator answered every call, but the resource the check polls never reports the terminal status AWS converges it to, so the polling loop never exits. None of these six is marked ALREADY FILED or OWNED by the suite.

Shared mechanism (Hypothesis, stated once and not repeated per cause): the store models each status transition as a change owed to the clock (`PendingStatus::in_a_moment`, due `STATUS_TRANSITION_DELAY` later) that is materialised only when a LATER request arrives (`Store::settle_due_transitions`, services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs:507, called at the top of each handler). A client therefore pays at least one full poll interval per transition; these checks poll at 10–30 s intervals, so a flow that chains three or more transitions cannot complete inside one check's budget. The model and the recordings state the statuses, not this latency, hence Hypothesis.

Cause 1 of 6: the CopyDBSnapshot round trip never completes.
AWS: a DB instance converges to `available` (DBInstance.DBInstanceStatus), a DB snapshot of it converges to `available`, and the copy CopyDBSnapshot answers in `copying` converges to `available`; deleting the source then leaves the copy `available`. Each terminal value is reached without further client action, so a client that polls DescribeDBInstances/DescribeDBSnapshots sees it.
Emulator: the chain instance→snapshot→copy→source-deleted never gets through all four waits; the check is killed at 60 s and the copy is never observed `available`.
Why: a client that creates a snapshot copy and waits for it to be restorable waits forever — it can never learn that the copy is usable, and nothing is refused to tell it otherwise.
Fix: the pending-status schedule and settling of DB-snapshot/DB-instance records in the rds state store, and the transition schedules CreateDBInstance / CreateDBSnapshot / CopyDBSnapshot install.
Traces: rds.copy-db-snapshot-round-trip.

Cause 2 of 6: the CopyDBClusterSnapshot round trip never completes.
AWS: a DB cluster converges to `available`, a cluster snapshot of it to `available`, and the copy to `available`; DescribeDBClusterSnapshots reports the copy as an independent snapshot.
Emulator: the chain cluster→member instance→cluster snapshot→copy never gets through its waits; the check is killed at 60 s.
Why: a client cannot establish that an Aurora cluster snapshot copy exists and is restorable.
Fix: the pending-status schedule and settling of DB-cluster and DB-cluster-snapshot records in the rds state store, and the schedules CreateDBCluster / CreateDBClusterSnapshot / CopyDBClusterSnapshot install.
Traces: rds.copy-db-cluster-snapshot-round-trip.

Cause 3 of 6: the snapshot export-task lifecycle never leaves its initial status in time.
AWS: Smithy ExportTask.Status documents the progress statuses STARTING, IN_PROGRESS, COMPLETE, CANCELING, CANCELED, FAILED; a started export leaves STARTING on its own and is then cancellable, so DescribeExportTasks eventually reports IN_PROGRESS/COMPLETE/FAILED.
Emulator: the flow instance→snapshot→StartExportTask→DescribeExportTasks does not reach a status in {IN_PROGRESS, COMPLETE, FAILED} before the check is killed at 60 s.
Why: a client polling an export task to completion never gets a terminal or in-progress status, so a snapshot export can never be reported as finished or cancelled.
Fix: the export-task status schedule in StartExportTask/DescribeExportTasks and the settling of export-task records in the rds state store.
Traces: rds.export-task-lifecycle.

Cause 4 of 6: the retained-automated-backup flow never becomes observable.
AWS: DeleteDBInstance with DeleteAutomatedBackups=false retains the instance's automated backup; the instance then converges out of existence (DescribeDBInstances answers DBInstanceNotFound) and DescribeDBInstanceAutomatedBackups reports the retained backup, which DeleteDBInstanceAutomatedBackup by DBInstanceAutomatedBackupsArn then removes, after which a repeat delete is refused DBInstanceAutomatedBackupNotFound with the message `Automated backups could not be found for: <DbiResourceId>` (fixed text `Automated backups could not be found for: `, variable part the request's/instance's DbiResourceId).
Emulator: the chain instance available→instance deleted→retained backup describable→backup deleted never completes; the check is killed at 60 s, so neither the retained backup nor the refusal is ever reached.
Why: a client that relies on retained automated backups cannot see one appear or disappear, and never gets the not-found refusal that tells it the backup is gone.
Fix: the DB-instance deletion/settling schedule and the automated-backup records in the rds state store, plus DescribeDBInstanceAutomatedBackups / DeleteDBInstanceAutomatedBackup.
Traces: rds.delete-automated-backup-by-arn.

Cause 5 of 6: ModifyDBInstance(ApplyImmediately=true) superseding a deferred class change never converges.
AWS: an ApplyImmediately=false modify parks the change in PendingModifiedValues; a later ApplyImmediately=true modify applies both the class and the storage increase and leaves PendingModifiedValues empty, with the instance converging back to `available`.
Emulator: the flow create→available→deferred modify→immediate modify→available does not get through its waits; the check is killed at 60 s, so the post-modify PendingModifiedValues is never read.
Why: a client resizing an instance cannot tell when the change has landed, so it cannot distinguish an applied change from a parked one.
Fix: the modify/settling schedule for DB-instance records (pending-modified-values application) in the rds state store and ModifyDBInstance.
Traces: rds.modify-apply-immediately-true-clears-a-prior-deferred-class-change.

Cause 6 of 6: the concurrent-FailoverDBCluster single-winner flow never converges.
AWS: of two FailoverDBCluster calls on one Aurora cluster, with the second issued while the first is in flight, exactly one succeeds and the other is refused InvalidDBClusterStateFault, and the cluster returns to `available`.
Emulator: the flow cluster→two member instances→concurrent failovers→cluster `available` does not get through its waits; the check is killed at 60 s, so neither the single-winner outcome nor the return to `available` is ever observed.
Why: a client cannot establish that failover is serialized per cluster, and a cluster it failed over never reports itself available again within any reasonable poll.
Fix: the cluster failover status schedule and settling of DB-cluster/member records in the rds state store and FailoverDBCluster.
Traces: rds.concurrent-failover-requests-single-winner.

## Evidence
All six checks fail with the identical detail from the behavioral suite: `Error: Check timed out after 60.0s`. No HTTP error, no error code and no refusal is produced — the emulator keeps answering the polling DescribeDB* calls, but the polled status never becomes the terminal value the check waits for.

Expected (AWS): each polled resource reports its terminal status — DBInstanceStatus `available` then DBInstanceNotFound after delete, DBSnapshot/DBClusterSnapshot Status `available`, ExportTask Status in {IN_PROGRESS, COMPLETE, FAILED} after STARTING, a retained automated backup describable then DBInstanceAutomatedBackupNotFound (`Automated backups could not be found for: <DbiResourceId>`), PendingModifiedValues empty after an ApplyImmediately=true modify, and the cluster back to `available` after a failover — so a polling client exits its loop.

Actual (emulator): none of the six chains reaches its terminal state before the check is killed at 60 s. Relevant code path: `Store::settle_due_transitions` (services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs:507) materialises only transitions already due, and `PendingStatus::in_a_moment` (same file, around line 1552) schedules each transition `STATUS_TRANSITION_DELAY` into the future, so each transition in a chain is paid for with a whole client poll interval (10–30 s in these checks).

Precedent for this failure class in this project: 20261024T182404Z-elbv2-listener-rule-priority-collision-update-path-never.md (`never converges: the check never reaches a terminal result inside the harness budget (60s timeout)`), since fixed.

## Steps to Reproduce
Replay the six checks named in trace_ids against the emulator:

1. rds.copy-db-snapshot-round-trip — CreateDBInstance(mysql, db.t3.micro) → poll DescribeDBInstances for `available` → CreateDBSnapshot → poll DescribeDBSnapshots for `available` → CopyDBSnapshot → poll DescribeDBSnapshots for the copy `available` → DeleteDBSnapshot(source) → poll for DBSnapshotNotFound.
2. rds.copy-db-cluster-snapshot-round-trip — CreateDBCluster(aurora-mysql) + CreateDBInstance member → poll DescribeDBClusters for `available` → CreateDBClusterSnapshot → poll for `available` → CopyDBClusterSnapshot → poll for the copy `available`.
3. rds.export-task-lifecycle — instance `available` → CreateDBSnapshot `available` → StartExportTask(SourceArn=snapshot ARN, S3BucketName, IamRoleArn, KmsKeyId) → poll DescribeExportTasks until Status is IN_PROGRESS/COMPLETE/FAILED → CancelExportTask.
4. rds.delete-automated-backup-by-arn — instance `available` → DeleteDBInstance(SkipFinalSnapshot=true, DeleteAutomatedBackups=false) → poll until DescribeDBInstances answers DBInstanceNotFound → poll DescribeDBInstanceAutomatedBackups(DbiResourceId) for the retained backup → DeleteDBInstanceAutomatedBackup(DBInstanceAutomatedBackupsArn) → poll until gone → repeat the delete and read the refusal.
5. rds.modify-apply-immediately-true-clears-a-prior-deferred-class-change — CreateDBInstance(postgres, db.t3.micro, 20 GiB) → `available` → ModifyDBInstance(DBInstanceClass=db.t3.small, ApplyImmediately=false) → ModifyDBInstance(class + AllocatedStorage=25, ApplyImmediately=true) → poll back to `available` and read PendingModifiedValues.
6. rds.concurrent-failover-requests-single-winner — CreateDBCluster(aurora-mysql) → `available` → create writer and reader members concurrently → two FailoverDBCluster calls with the second issued while the first is in flight → poll the cluster back to `available`.

Each run is killed at the 60 s per-check budget before its last poll succeeds.
