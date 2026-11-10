Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-table-lifecycle-core
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-table-lifecycle-core dynamodb-table-lifecycle-with-gsi-and-ttl dynamodb-on-demand-backup-and-restore dynamodb-backup-create-list-restore-cleanup ddbctl-backup-on-creating-table-and-restore-name-collision
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|DescribeTimeToLive|2xx->2xx|$.TimeToLiveDescription.TimeToLiveStatus
Issue-Key: dynamodb|DescribeBackup|2xx->2xx|$.BackupDescription.BackupDetails.BackupStatus
Issue-Key: dynamodb|ListBackups|2xx->2xx|$.BackupSummaries[TableName="sim2real-probe-dynamodb-obr-src-a3f944ed"].BackupStatus
Issue-Key: dynamodb|DescribeBackup|2xx->2xx|$.BackupDescription.SourceTableFeatureDetails.TimeToLiveDescription.TimeToLiveStatus
Issue-Key: dynamodb|DeleteBackup|2xx->2xx|$.BackupDescription.SourceTableFeatureDetails.TimeToLiveDescription.TimeToLiveStatus
Issue-Key: dynamodb|DescribeContinuousBackups|2xx->2xx|$.ContinuousBackupsDescription.PointInTimeRecoveryDescription.PointInTimeRecoveryStatus
Issue-Commit: 78d5907a1724d3304a3021142ffd4f12ae7136a6

# DynamoDB asynchronous resource states never settle to their terminal value

> _Note: this issue was identified against the code base at commit `78d5907a1724d3304a3021142ffd4f12ae7136a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTimeToLive

## Description
Several DynamoDB resources that AWS transitions through a transient state to a terminal one are left parked in the transient state (or in the wrong terminal state) by the emulator. Every one of these is a value a client polls on, so the divergence is not cosmetic: a conventional wait loop never exits. 6 causes, all dynamodb.

Cause 1 of 6: DescribeTimeToLive reports TimeToLiveStatus 'ENABLING' forever after a successful UpdateTimeToLive(Enabled=true).
AWS: Smithy: TimeToLiveStatus is an enum with values [ENABLING, DISABLING, ENABLED, DISABLED]; UpdateTimeToLive returns the requested TimeToLiveSpecification and the table's TTL then becomes ENABLED. The recording shows DescribeTimeToLive answering 200 with TimeToLiveDescription.TimeToLiveStatus = 'ENABLED' after the enable call.
Emulator: 200 with TimeToLiveDescription.TimeToLiveStatus = 'ENABLING' — the transient state is written and never advanced.
Why: a client that polls DescribeTimeToLive until the status is ENABLED (the documented way to confirm TTL is on) loops until its own timeout; TTL is never observably enabled even though the enable succeeded.
Fix: the table's TTL state — after UpdateTimeToLive is accepted, settle TimeToLiveStatus to ENABLED (and to DISABLED for a disable) rather than leaving the ENABLING/DISABLING transient value in the store.
Traces: dynamodb-table-lifecycle-core#task#8 (cluster of 2, also dynamodb-table-lifecycle-with-gsi-and-ttl#task#5).

Cause 2 of 6: DescribeBackup reports BackupDetails.BackupStatus 'CREATING' forever.
AWS: Smithy: BackupStatus is an enum [CREATING, DELETED, AVAILABLE]. The recording shows DescribeBackup answering 200 with BackupStatus 'AVAILABLE' once an on-demand backup has been taken.
Emulator: 200 with BackupStatus 'CREATING'.
Why: RestoreTableFromBackup is only legal against an AVAILABLE backup, and clients gate the restore on that status; the backup never becomes usable from the client's point of view.
Fix: the CreateBackup/DescribeBackup state — settle BackupStatus to AVAILABLE once the backup record is complete.
Traces: dynamodb-on-demand-backup-and-restore#task#7.

Cause 3 of 6: ListBackups reports the same backup's BackupStatus as 'CREATING'.
AWS: Smithy: BackupSummary.BackupStatus targets the same BackupStatus enum; the recording shows 'AVAILABLE' in the summary for the backup of the source table.
Emulator: 'CREATING' in BackupSummaries for that table.
Why: a client that enumerates backups and filters for AVAILABLE ones finds none, so restore-from-latest-backup flows select nothing.
Fix: the ListBackups projection reads the same backup state as cause 2 — one settled status must be visible from both the Describe and the List path.
Traces: dynamodb-on-demand-backup-and-restore#task#8.

Cause 4 of 6: DescribeBackup's captured source-table feature details report TTL as 'ENABLING'.
AWS: Smithy: BackupDescription.SourceTableFeatureDetails.TimeToLiveDescription.TimeToLiveStatus is the TimeToLiveStatus enum snapshotted at backup time. The recording shows 'ENABLED' because the source table's TTL was enabled before the backup was taken.
Emulator: 'ENABLING'.
Why: the backup's recorded feature set misstates whether the restored table will have TTL on, so a client validating that a backup captured TTL concludes it did not.
Fix: this is the same unsettled TTL state as cause 1, read through the backup snapshot — the snapshot must copy the settled status.
Traces: dynamodb-backup-create-list-restore-cleanup#task#7.

Cause 5 of 6: DeleteBackup's returned BackupDescription repeats the 'ENABLING' TTL snapshot.
AWS: Smithy: DeleteBackup returns a BackupDescription with the same SourceTableFeatureDetails shape; the recording shows TimeToLiveStatus 'ENABLED'.
Emulator: 'ENABLING'.
Why: the delete response is the last record a client keeps of a deleted backup; it misreports the captured feature set.
Fix: same snapshot path as cause 4, on the DeleteBackup response projection.
Traces: dynamodb-backup-create-list-restore-cleanup#task#10.

Cause 6 of 6: DescribeContinuousBackups reports PointInTimeRecoveryStatus 'ENABLED' for a table whose UpdateContinuousBackups call AWS refused.
AWS: Smithy: PointInTimeRecoveryDescription.PointInTimeRecoveryStatus documents exactly two states, ENABLED (point in time recovery is enabled) and DISABLED. The recording shows the enable attempt on a table that is not yet usable being refused, so the subsequent DescribeContinuousBackups answers 200 with PointInTimeRecoveryStatus 'DISABLED'.
Emulator: 200 with 'ENABLED' — the refused (see the companion issue on the non-ACTIVE-table guards) or at least non-effective enable is nonetheless recorded as applied.
Why: a client reads back PITR as on for a table that has no continuous backups, then relies on RestoreTableToPointInTime being available; the state the emulator reports is not the state it can serve.
Fix: the UpdateContinuousBackups handler and the continuous-backups state — only commit PointInTimeRecoveryStatus when the update is actually accepted for that table.
Traces: ddbctl-backup-on-creating-table-and-restore-name-collision#task#9 (cluster of 2, also task#13 of the same fixture).

Related open issues: '20261025T015306Z-dynamodb-describetimetolive-stub.md' filed DescribeTimeToLive as an unimplemented stub; the operation now answers 200, so cause 1 is the next layer and is not covered by it.

## Evidence
Divergence 36 (DescribeTimeToLive, 200->200): $.TimeToLiveDescription.TimeToLiveStatus expected 'ENABLED', got 'ENABLING' (cluster of 2).
Divergence 27 (DescribeBackup, 200->200): $.BackupDescription.BackupDetails.BackupStatus expected 'AVAILABLE', got 'CREATING'.
Divergence 28 (ListBackups, 200->200): $.BackupSummaries[TableName="...obr-src..."].BackupStatus expected 'AVAILABLE', got 'CREATING'.
Divergence 16 (DescribeBackup, 200->200): $.BackupDescription.SourceTableFeatureDetails.TimeToLiveDescription.TimeToLiveStatus expected 'ENABLED', got 'ENABLING'.
Divergence 17 (DeleteBackup, 200->200): same member, expected 'ENABLED', got 'ENABLING'.
Divergence 3 (DescribeContinuousBackups, 200->200): $.ContinuousBackupsDescription.PointInTimeRecoveryDescription.PointInTimeRecoveryStatus expected 'DISABLED', got 'ENABLED' (cluster of 2).
Model: smithy_query(shape=TimeToLiveStatus) -> [ENABLING, DISABLING, ENABLED, DISABLED]; shape=BackupStatus -> [CREATING, DELETED, AVAILABLE]; shape=PointInTimeRecoveryDescription -> PointInTimeRecoveryStatus doc lists only ENABLED/DISABLED as the states a client observes.

## Steps to Reproduce
1. Replay dynamodb-table-lifecycle-core#task#8: UpdateTimeToLive(Enabled=true) then DescribeTimeToLive -> status 'ENABLING' instead of 'ENABLED'.
2. Replay dynamodb-on-demand-backup-and-restore#task#7 and #task#8: CreateBackup then DescribeBackup/ListBackups -> BackupStatus 'CREATING' instead of 'AVAILABLE'.
3. Replay dynamodb-backup-create-list-restore-cleanup#task#7 and #task#10: DescribeBackup/DeleteBackup -> SourceTableFeatureDetails TTL status 'ENABLING' instead of 'ENABLED'.
4. Replay ddbctl-backup-on-creating-table-and-restore-name-collision#task#9: DescribeContinuousBackups after a rejected enable -> PointInTimeRecoveryStatus 'ENABLED' instead of 'DISABLED'.
