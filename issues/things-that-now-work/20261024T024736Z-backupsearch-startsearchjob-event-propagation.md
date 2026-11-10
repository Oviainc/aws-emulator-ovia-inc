Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# Search scope never resolves against the AWS Backup emulator's recovery points, so every search job fails with an empty scope

_Filed: 2026-10-24T02:47:36Z_
_Operation: StartSearchJob | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A BackupSearch search job scans the recovery points that AWS Backup holds (those whose backup index exists). In this repo the AWS Backup emulator (service-com-amazonaws-backup-cryocontrollerusermanager) already persists recovery points: `StartBackupJob` writes `RecoveryPoint` records into `state.recovery_points` / `recovery_points_by_vault`, and `ListRecoveryPointsByBackupVault` / `DescribeBackupVault` report them.

BackupSearch keeps its OWN `recovery_points: Vec<SearchJobBackupsResult>` list (src/state.rs) that nothing but a test ever writes to: no handler, and no cross-service hook, ever imports a recovery point created in the Backup emulator. As a result `recovery_points_in_scope` always resolves to the empty set for a real client, `scan_outcome` always owes the job FAILED with StatusMessage "Search scope is empty. Revise search scope input and retry search job", `ListSearchJobBackups`/`ListSearchJobResults` always answer an empty page, and `StartSearchResultExportJob` can never be reached from a COMPLETED job (only from a STOPPED one) because no scan can ever complete.

Expected: after StartBackupJob in the Backup emulator produces a recovery point of a searchable resource type (S3/EBS) with its index enabled, a StartSearchJob whose SearchScope selects that recovery point (by BackupResourceTypes, BackupResourceArns, SourceResourceArns or a creation-time window) must resolve to it: SearchScopeSummary.TotalRecoveryPointsToScanCount > 0, the job settles to COMPLETED, and ListSearchJobBackups reports that recovery point.

## Evidence

Well-established service semantics: AWS Backup search operates over recovery points created by AWS Backup (BackupResourceArns in SearchScope are recovery-point ARNs, SourceResourceArns are the protected resources); the Smithy model's `SearchJobBackupsResult` carries `BackupResourceArn`, `SourceResourceArn`, `ResourceType`, `BackupCreationTime` and `IndexCreationTime`, i.e. exactly the Backup-plane record.

Emulator: services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/state.rs documents that `recovery_points` "stays EMPTY" in a running emulator because "nothing in THIS service creates one", and `recovery_points_in_scope` (state.rs:467) filters that empty vector; src/ops/start_search_job.rs:275-290 therefore always takes the `scan_outcome(0)` = FAILED/SCOPE_EMPTY branch. The peer crate services/service-com-amazonaws-backup-cryocontrollerusermanager/src/state.rs:713-718 shows the recovery points that exist in the same emulator process and are never consulted.

Test coverage: tests/all/search_job_outcome_and_paging.rs seeds `recovery_points` directly into the engine state to reach the COMPLETED branch — i.e. the COMPLETED path is only reachable by in-process seeding, never by any sequence of API calls. No test asserts cross-service propagation from StartBackupJob.

## Steps to Reproduce

1. Boot the emulator with both the backup and backup-search services mounted.
2. backup: CreateBackupVault(BackupVaultName="vault"), then StartBackupJob for an S3 resource (with the backup index enabled) and wait for the recovery point to become AVAILABLE; confirm ListRecoveryPointsByBackupVault reports it.
3. backup-search: StartSearchJob(SearchScope={"BackupResourceTypes":["S3"]}) and poll GetSearchJob until the status is terminal.
4. Emulator: Status=FAILED with StatusMessage "Search scope is empty…", SearchScopeSummary.TotalRecoveryPointsToScanCount=0, and ListSearchJobBackups returns an empty Results list. Expected: the scope resolves to the recovery point created in step 2, the job settles to COMPLETED, and ListSearchJobBackups reports that recovery point.
