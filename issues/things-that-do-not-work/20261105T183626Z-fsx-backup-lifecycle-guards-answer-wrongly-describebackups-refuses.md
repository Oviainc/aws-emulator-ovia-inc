Issue-Source: trace-sweep-divergence
Issue-Trace: fsx-backup-lifecycle-guards
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: fsx-backup-lifecycle-guards
Issue-Shape-Id: com.amazonaws.fsx#AWSSimbaAPIService_v20180301
Issue-Key: fsx|DescribeBackups|2xx->4xx|$.Backups
Issue-Key: fsx|DescribeBackups|2xx->4xx|status
Issue-Key: fsx|DeleteBackup|4xx->4xx|$.Message
Issue-Commit: df9e8b985d3a6bd2268b23dac218237a4678e4c0

# fsx: backup-lifecycle guards answer wrongly — DescribeBackups refuses a filtered listing that selects nothing with 400, and DeleteBackup's BackupNotFound Message wraps prose around the id

> _Note: this issue was identified against the code base at commit `df9e8b985d3a6bd2268b23dac218237a4678e4c0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeBackups

## Description
Two net-new root causes in the bootstrapped `fsx` emulator's backup slice, both grounded in replayed wire steps of the fixture `fsx-backup-lifecycle-guards`. The two open `fsx` issues (20261105T044046Z-fsx-2-net-new-root-causes-updatefilesystem-enforces.md, which covers UpdateFileSystem's storage-capacity floor and CreateDataRepositoryAssociation's 501 stub, and 20261105T151930Z-fsx-openzfs-file-systems-report-no-diskiopsconfiguration-the.md, which covers the missing OpenZFS DiskIopsConfiguration default) mention neither the DescribeBackups empty-selection status nor the DeleteBackup BackupNotFound message, so neither cause is covered by them. Both causes live in the backup read/delete path and must BOTH be fixed before this issue is closed.

=== Cause 1 of 2: DescribeBackups turns a selection that matches no backup into a 400 instead of 200 with an empty Backups list ===
Fixture/step: fsx-backup-lifecycle-guards#task#4 (operation DescribeBackups).

AWS: Smithy model — `DescribeBackups` output member `Backups` targets the `Backups` list with `@length min 0, max 50` ("An array of backups"), i.e. the EMPTY list is a modelled, successful answer. The model attaches not-found semantics to exactly one input path: `DescribeBackupsRequest$BackupIds` — "The IDs of the backups that you want to retrieve. This parameter value overrides any filters. If any IDs aren't found, a `BackupNotFound` error occurs." `DescribeBackupsRequest$Filters` documents only which names it accepts ("The supported names are file-system-id, backup-type, file-system-type, and volume-id") and says nothing about erroring when a filter selects nothing; `Filter` itself is documented as "A filter used to restrict the results of describe calls". Recorded behavior: the replayed step's recorded AWS response is HTTP 200 with `Backups` present and EMPTY (`[]`). So a listing that names no `BackupIds` and whose filters select no stored backup is a 200 whose `Backups` is an empty list — and `BackupNotFound`/`FileSystemNotFound`/`VolumeNotFound` are not reachable from that path.

Emulator: HTTP 400, with no `Backups` member in the body at all (the replay reports `status: expected 200, got 400` and `$.Backups: missing (expected [], got None)`). The handler validates filter VALUES against live state before it selects anything: services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/src/ops/describe_backups.rs lines 218-229 call `unknown_filter_value(...)` for every filter and return its refusal, and that helper (same file, lines 326-353) manufactures a client error whenever a filter value names nothing the store holds — `FileSystemNotFound` with Message "File system '<FileSystemId>' does not exist." when neither a live file system nor any backup of it is held, and `VolumeNotFound` with Message "Volume '<VolumeId>' does not exist." UNCONDITIONALLY for any value of the `volume-id` filter (the code comments that this slice stores no volumes, so "no value of this filter can name one"). Those are the only 400s this handler can produce for a listing that names no `BackupIds`, besides `BadRequest` for an unsupported filter name or a foreign `NextToken`. The result is that "nothing matched" is reported as a client error rather than as an empty page, which is exactly the divergence the step shows.

Why: enumerating backups by filter is how a client discovers whether anything exists — a backup-retention sweep, a "are all backups of this file system gone?" teardown check, a paginator feeding a list into a UI. Against AWS those all read `Backups == []` and conclude "none"; against the emulator they take a 400 BackupNotFound/FileSystemNotFound/VolumeNotFound exception. SDK paginators raise rather than yield an empty page, teardown and retention logic treats a transient-looking client error as a failure and either retries forever or aborts leaving resources behind, and the very assertion a lifecycle guard makes ("after the delete, the listing is empty") becomes impossible to express.

Fix: the operation's handler — `DescribeBackups::handle` in services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/src/ops/describe_backups.rs. Filter values must be SELECTORS, not existence assertions: drop the `unknown_filter_value` pre-check (and the helper) so a filter naming an unknown file system, or any `volume-id`, simply matches no stored record and the operation answers 200 with an empty `Backups` list and no `NextToken`. `BackupNotFound` must stay reachable only from the `BackupIds` branch, which the model does tie it to. Rejecting an unsupported filter NAME with `BadRequest` is a separate, modelled behavior and is not what this cause asks to change.

Traces: fsx-backup-lifecycle-guards#task#4.

=== Cause 2 of 2: DeleteBackup's BackupNotFound Message is a sentence about the id instead of the bare BackupId ===
Fixture/step: fsx-backup-lifecycle-guards#task#5 (operation DeleteBackup).

AWS: Smithy model — `DeleteBackup` declares `BackupNotFound` (HTTP 400, client error) whose single member `Message` targets `ErrorMessage` (`@length 1..256`); the shape's doc is "No Amazon FSx backups were found based upon the supplied parameters." The model does not state the Message TEMPLATE, so for the wording this is recorded behavior only: the recorded 400 response for a DeleteBackup naming a backup that does not exist carries `Message` equal to the requested `BackupId` and NOTHING else — there is no fixed text in this message, its entire content is the variable `BackupId` taken verbatim from the request (in the replayed recording that value happened to be `backup-04c8e1b7fa63d9502`; the id is a per-run value, the requirement is "the requested BackupId alone"). This emulator already encodes the same FSx convention for the sibling delete path: services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/src/state.rs lines 532-541, `deleted_file_system_not_found_message`, documents that DeleteFileSystem's recorded `FileSystemNotFound$Message` template "is the bare id, with no sentence around it" and returns exactly that. The status (400) and the error code (`BackupNotFound`) already agree; only the message text diverges.

Emulator: HTTP 400 with `Message` = "Backup '<BackupId>' does not exist." — the fixed text "Backup '", "' does not exist." wrapped around the requested id. It comes from `backup_not_found_message` (services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/src/state/backups.rs lines 416-418, `format!("Backup '{backup_id}' does not exist.")`), which services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/src/ops/delete_backup.rs line 207 uses for the not-found branch at lines 203-210.

Why: an error message is part of the response contract. A client that surfaces or logs the message shows wording AWS never emits; worse, tooling that parses the message to recover WHICH backup the server rejected (a cleanup loop that extracts the id from the failure, a test asserting the id it asked about is named, a log-scraping alarm matching on the recorded form) finds a different string shape and either fails to extract the id or stops matching, so a genuine not-found is misclassified.

Fix: the operation's error construction — give `DeleteBackup` its own `BackupNotFound$Message` constructor in services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/src/state/backups.rs, returning the requested `BackupId` alone (clamped by `within_error_message_length` as the other constructors are), exactly as `deleted_file_system_not_found_message` already does for DeleteFileSystem, and use it in delete_backup.rs's not-found branch. Leave `backup_not_found_message`'s sentence form in place for the operations whose own recordings use it (DescribeBackups' BackupIds path), rather than changing the shared helper for every caller.

Traces: fsx-backup-lifecycle-guards#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator up: /__admin/health 200, data listener accepting). Both steps below are `verdict: mismatch`, neither carries a `stub_path` and neither is `owned_by` an existing issue.

Cause 1 — fixture fsx-backup-lifecycle-guards, step task#4, operation DescribeBackups
  divergence_key: service=fsx|DescribeBackups|status=200->400|keys=$.Backups,status
  Expected (recorded AWS): HTTP 200, body with Backups = []  (an empty, present list)
  Actual (emulator):       HTTP 400, body with NO Backups member
  detail: "status: expected 200, got 400" / "$.Backups: missing (expected [], got None)"
  Code: services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/src/ops/describe_backups.rs
        lines 218-229 (the per-filter `unknown_filter_value` pre-check, applied before any record is
        selected) and lines 326-353 (`unknown_filter_value`: FileSystemNotFound when the store holds
        neither the named file system nor a backup of it; VolumeNotFound for ANY `volume-id` value).
        Those are the only 400s reachable for a listing that names no BackupIds, apart from BadRequest
        for an unsupported filter name (state/backups.rs lines 432-438) or a foreign NextToken
        (describe_backups.rs lines 254-266).

Cause 2 — fixture fsx-backup-lifecycle-guards, step task#5, operation DeleteBackup
  divergence_key: service=fsx|DeleteBackup|status=400->400|keys=$.Message
  Expected (recorded AWS): HTTP 400, BackupNotFound, Message = the requested BackupId alone
                           (the recording's value: 'backup-04c8e1b7fa63d9502' — the id is the request's
                           value, the template is "the BackupId, nothing else")
  Actual (emulator):       HTTP 400, Message = "Backup 'backup-04c8e1b7fa63d9502' does not exist."
                           (fixed text "Backup '" + "' does not exist." around the request's id)
  detail: "$.Message: value_mismatch (expected 'backup-04c8e1b7fa63d9502', got \"Backup 'backup-04c8e1b7fa63d9502' does not exist.\")"
  Code: services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/src/state/backups.rs lines
        416-418 (`backup_not_found_message` -> format!("Backup '{backup_id}' does not exist.")), used by
        services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/src/ops/delete_backup.rs lines
        203-210. Contrast the already-correct sibling convention at
        services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/src/state.rs lines 532-541
        (`deleted_file_system_not_found_message` — the bare id, "with no sentence around it").

Model citations used: smithy_query(operation="DescribeBackups,DeleteBackup") — DescribeBackups output
Backups (@length 0..50), errors BackupNotFound/BadRequest/FileSystemNotFound/VolumeNotFound (all HTTP 400),
DeleteBackup errors incl. BackupNotFound (HTTP 400); smithy_query(shape="BackupNotFound,DescribeBackupsRequest")
— BackupNotFound$Message -> ErrorMessage @length 1..256, BackupIds doc "If any IDs aren't found, a
BackupNotFound error occurs", Filters doc "The supported names are file-system-id, backup-type,
file-system-type, and volume-id".

## Steps to Reproduce
Cause 1 (DescribeBackups empty selection -> 400):
1. Point an FSx client at the emulator.
2. Call DescribeBackups with NO BackupIds and a Filters entry that selects nothing the store holds — e.g. Filters=[{'Name':'file-system-id','Values':['fs-0000000000000000a']}] for a file system id no record and no backup references, or Filters=[{'Name':'volume-id','Values':['fsvol-0000000000000000a']}] (the emulator refuses ANY volume-id value).
3. AWS answers HTTP 200 with Backups = [] and no NextToken. The emulator answers HTTP 400 (FileSystemNotFound / VolumeNotFound) with no Backups member.
4. Also confirm the unchanged behaviors: DescribeBackups(BackupIds=['backup-<unknown>']) must still be 400 BackupNotFound naming that id, a filter NAME outside {file-system-id, backup-type, file-system-type, volume-id} must still be 400 BadRequest, and a filter that DOES match stored backups must still return them.

Cause 2 (DeleteBackup BackupNotFound message):
5. Call DeleteBackup(BackupId='backup-<17 hex digits that name no stored backup>').
6. AWS answers HTTP 400 BackupNotFound whose Message is the requested BackupId alone (no surrounding prose). The emulator answers HTTP 400 BackupNotFound with Message "Backup '<BackupId>' does not exist.".
7. Confirm the id in the message is the one from the REQUEST (repeat with a second bogus id and see it echoed), and that DescribeBackups' own BackupIds-path message is left as its recording requires.

Replay to verify the fix:
8. Re-replay the wire corpus and confirm fsx-backup-lifecycle-guards steps task#4 AND task#5 both pass (both causes must close; task#4 is this issue's Issue-Step, task#5 is listed in its trace ids).
