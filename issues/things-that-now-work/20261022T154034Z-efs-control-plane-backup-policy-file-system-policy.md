Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: efs.backup-policy-status-transitions efs.put-file-system-policy-lockout-rejected efs.update-file-system-throughput-mode-missing-param-rejected efs.update-file-system-protection-toggle efs.create-file-system-token-reuse-conflict efs.delete-file-system-lifecycle-transition
Issue-Shape-Id: com.amazonaws.efs#MagnolioAPIService_v20150201
Issue-Key: efs.backup-policy-status-transitions|run#7|AssertionError|assert initial_code == "PolicyNotFound", f"expected PolicyNotFound, got {initial_code!r}"
Issue-Key: efs.put-file-system-policy-lockout-rejected|run#10|AssertionError|assert code == "InvalidPolicyException", (
Issue-Key: efs.update-file-system-throughput-mode-missing-param-rejected|run#7|AssertionError|assert code == "BadRequest", f"expected BadRequest for a provisioned-mode update missing ProvisionedThroughputInMibps, got {code!r}"
Issue-Key: efs.update-file-system-protection-toggle|run#6|KeyError|initial = efs.describe_file_systems(FileSystemId=fs_id)["FileSystems"][0]["FileSystemProtection"]
Issue-Key: efs|CreateFileSystem|FileSystemAlreadyExists
Issue-Key: efs.delete-file-system-lifecycle-transition|run#11|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Commit: db1fbaa8559703996072fdf323a0d84f8f863f33

# EFS control plane: backup-policy, file-system-policy and UpdateFileSystem paths answer 501 NotImplemented; FileSystemProtection absent; error messages not request-derived (6 causes)

> _Note: this issue was identified against the code base at commit `db1fbaa8559703996072fdf323a0d84f8f863f33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeBackupPolicy

## Description
Replay of the behavioral suite simparity-behavioral-aws-v1 against the emulator found six distinct EFS (service token `efs`, Smithy sdkId EFS / sigv4Name elasticfilesystem, api 2015-02-01, restJson1) root causes that are NOT covered by the scaffold-stub issues the report filed itself. THIS ISSUE HAS 6 CAUSES; all six must be fixed before it is closed. Two of them (Cause 5, Cause 6) are the behavioral-corpus evidence for defects already described from the wire corpus in the open issue 20261022T151224Z-elasticfilesystem-efs-not-found-and-already-exists-error-messages.md, and Cause 4 overlaps the open issue 20261022T151224Z-elasticfilesystem-efs-file-system-description-diverges-omitted-members-wrong.md; those issues are named here so the work can be merged rather than done twice.

Cause 1 of 6 — DescribeBackupPolicy / PutBackupPolicy are not implemented, so the backup-policy lifecycle is unreachable (check efs.backup-policy-status-transitions).
AWS: Smithy DescribeBackupPolicy = GET /2015-02-01/file-systems/{FileSystemId}/backup-policy, success 200, output BackupPolicyDescription{BackupPolicy}; modeled errors BadRequest (client, 400), FileSystemNotFound (client, 404), PolicyNotFound (client, 404), ValidationException (client, 400), InternalServerError (server, 500). PutBackupPolicy = PUT on the same URI, success 200, input member BackupPolicy required, output BackupPolicyDescription. BackupPolicy.Status is enum Status with values ENABLED|ENABLING|DISABLED|DISABLING, doc: ENABLING "EFS is turning on automatic backups", DISABLING "EFS is turning off automatic backups". Recorded behavior: a file system that never had a backup policy set answers DescribeBackupPolicy with 404 PolicyNotFound and NO Message member (rather than a synthesized DISABLED status); PutBackupPolicy{Status: ENABLED} answers 200 with Status in (ENABLING, ENABLED) and DescribeBackupPolicy then converges to ENABLED; PutBackupPolicy{Status: DISABLED} answers 200 with Status in (DISABLING, DISABLED) and converges to DISABLED.
Emulator: DescribeBackupPolicy raises error code 'NotImplemented' ("operation ... is scaffolded but not implemented in this emulator") where AWS raises PolicyNotFound at 404, so neither the not-found refusal nor any Put/Describe transition is reachable.
Why: a client that gates on backup protection (compliance checks that require BackupPolicy.Status == ENABLED, or that treat PolicyNotFound as "never configured") sees an unknown error class it cannot classify: 501/NotImplemented is neither the modeled 404 nor a retryable server condition, so the branch for "no policy yet" never runs and enabling backups is impossible.
Fix: the DescribeBackupPolicy and PutBackupPolicy handlers plus backup-policy state on the file-system record — store the policy per file system, refuse DescribeBackupPolicy with PolicyNotFound (404) while no policy exists, and have Put drive the Status enum to a settled ENABLED/DISABLED observable by Describe.
Traces: efs.backup-policy-status-transitions.

Cause 2 of 6 — PutFileSystemPolicy is not implemented, so the policy-lockout safety check and its bypass do not exist (check efs.put-file-system-policy-lockout-rejected).
AWS: Smithy PutFileSystemPolicy = PUT /2015-02-01/file-systems/{FileSystemId}/policy, success 200, input Policy (required, @length 1..20000) and BypassPolicyLockoutSafetyCheck (boolean, default false); modeled errors BadRequest (400), FileSystemNotFound (404), IncorrectFileSystemLifeCycleState (409), InvalidPolicyException (client, 400 — doc: "Returned if the FileSystemPolicy is malformed or contains an error ... Returned in the case of a policy lockout safety check error"), InternalServerError (500). Recorded behavior: a policy whose only statement Denies elasticfilesystem:PutFileSystemPolicy to Principal * on the file system's own ARN is refused with 400 InvalidPolicyException (recorded message example: 'This policy would prevent future FileSystemPolicy updates. Retry the request with the BypassPolicyLockoutSafetyCheck parameter to apply this policy.'); the refusal leaves no policy behind, so DescribeFileSystemPolicy then answers 404 PolicyNotFound with no Message; the identical policy with BypassPolicyLockoutSafetyCheck=true succeeds at 200 and the returned Policy parses equal to the submitted document.
Emulator: PutFileSystemPolicy answers error code 'NotImplemented' instead of InvalidPolicyException (400), so the lockout check, the bypass path and policy persistence are all absent.
Why: the lockout safety check is the guardrail that stops a caller from permanently bricking a file system's resource policy. A client that relies on being refused unless it deliberately passes BypassPolicyLockoutSafetyCheck gets an unclassifiable error here instead, and no policy workflow (put, re-read, bypass) can be exercised at all.
Fix: the PutFileSystemPolicy handler, its policy validation (lockout detection gated on BypassPolicyLockoutSafetyCheck) and the file-system policy state read back by DescribeFileSystemPolicy, which must refuse with PolicyNotFound (404) while no policy is stored.
Traces: efs.put-file-system-policy-lockout-rejected.

Cause 3 of 6 — UpdateFileSystem does not perform EFS's conditional-parameter validation: switching to provisioned throughput without ProvisionedThroughputInMibps must be refused as BadRequest, with no state change (check efs.update-file-system-throughput-mode-missing-param-rejected). The bare fact that UpdateFileSystem is a scaffold stub is already filed by this run's own stub issue for services/service-com-amazonaws-efs-magnolioapiservice-v20150201/src/ops/update_file_system.rs; the requirement recorded HERE and not covered there is the rejection class plus effect-absence.
AWS: Smithy UpdateFileSystem = PUT /2015-02-01/file-systems/{FileSystemId}, success code 202, input ThroughputMode (enum bursting|provisioned|elastic) and ProvisionedThroughputInMibps (double, @range min 1.0); modeled errors BadRequest (client, 400 — doc "Returned if the request is malformed or contains an error such as an invalid parameter value or a missing required parameter"), FileSystemNotFound (404), IncorrectFileSystemLifeCycleState (409), ThroughputLimitExceeded (400), TooManyRequests (429), InsufficientThroughputCapacity (503), InternalServerError (500). Recorded behavior: ThroughputMode=provisioned with ProvisionedThroughputInMibps omitted is refused 400 BadRequest (recorded message example: 'Provisioned throughput must be set for file systems that use provisioned throughput mode.'); after the refusal DescribeFileSystems still reports ThroughputMode=bursting and LifeCycleState=available; the same call WITH ProvisionedThroughputInMibps=1.0 succeeds and the file system settles at ThroughputMode=provisioned.
Emulator: the call fails with error code 'NotImplemented' instead of BadRequest, so a client cannot distinguish "you forgot a conditionally required parameter" from "this API does not exist", and the accept/refuse boundary of UpdateFileSystem is untested.
Fix: the UpdateFileSystem handler's request validation — require ProvisionedThroughputInMibps whenever ThroughputMode resolves to provisioned, refuse with BadRequest (400) and leave the stored file-system record (ThroughputMode, LifeCycleState) untouched on the refusal path.
Traces: efs.update-file-system-throughput-mode-missing-param-rejected.

Cause 4 of 6 — FileSystemDescription omits FileSystemProtection, so the replication-overwrite-protection toggle is unobservable (check efs.update-file-system-protection-toggle). Overlaps the open issue 20261022T151224Z-elasticfilesystem-efs-file-system-description-diverges-omitted-members-wrong.md (its Causes 1/2/4 record the same omission from the wire corpus); the extra requirement recorded HERE is the UpdateFileSystemProtection round trip.
AWS: Smithy FileSystemDescription (the DescribeFileSystems list element and the UpdateFileSystem output) models FileSystemProtection -> FileSystemProtectionDescription{ReplicationOverwriteProtection}, enum ENABLED|DISABLED|REPLICATING, whose doc states "Replication overwrite protection is ENABLED by default". UpdateFileSystemProtection = PUT /2015-02-01/file-systems/{FileSystemId}/protection, idempotent, success 200, output FileSystemProtectionDescription. Recorded behavior: a freshly created file system reports FileSystemProtection.ReplicationOverwriteProtection = ENABLED on DescribeFileSystems; UpdateFileSystemProtection echoes the requested value and an independent DescribeFileSystems read then observes DISABLED, and ENABLED again after the reverse toggle.
Emulator: the DescribeFileSystems element has no FileSystemProtection member at all — the botocore client raises KeyError: 'FileSystemProtection' on the first read, before any update is attempted — so the default value and both toggle directions are unobservable.
Why: replication-overwrite protection decides whether a file system may be a replication destination and whether it is writeable. A client reading fs['FileSystemProtection']['ReplicationOverwriteProtection'] gets None/KeyError instead of a value, so protection checks take the unknown branch, and a read-after-write loop that waits for the flipped value never terminates.
Fix: the shared EFS FileSystemDescription serializer must always emit FileSystemProtection from the stored record, and the UpdateFileSystemProtection handler must persist ReplicationOverwriteProtection into that record so create/describe/update cannot drift.
Traces: efs.update-file-system-protection-toggle.

Cause 5 of 6 — the FileSystemAlreadyExists 409 message does not name the file system that already holds the creation token (check efs.create-file-system-token-reuse-conflict). This is the behavioral-corpus evidence for Cause 1 of the open issue 20261022T151224Z-elasticfilesystem-efs-not-found-and-already-exists-error-messages.md.
AWS: Smithy error FileSystemAlreadyExists, errorKind client, httpError 409, required members ErrorCode and FileSystemId. The message rule is request-derived: it names the EXISTING file system's id and the CreationToken from the request, each in single quotes — recorded example "File system 'fs-e220a8397b1dcdaf' already exists with creation token 'sim2real-efs-fs-e97ed7afbe3e'". The particular id is environment-specific; the construction is the requirement.
Emulator: status 409 and the error class match, but Message is 'File system already exists with CreationToken sim2real-efs-fs-e97ed7afbe3e' — the conflicting file-system id is absent and the token is unquoted.
Why: the only actionable datum in this refusal is which file system already owns the token; a client (or an operator reading the log line) that would adopt or describe that file system cannot, because the id never appears.
Fix: the CreateFileSystem conflict path's error construction — build the FileSystemAlreadyExists message from the existing record's FileSystemId and the request's CreationToken.
Traces: efs.create-file-system-token-reuse-conflict.

Cause 6 of 6 — after a successful DeleteFileSystem, the FileSystemNotFound 404 message is not constructed the way AWS builds it (check efs.delete-file-system-lifecycle-transition). This is the behavioral-corpus evidence for Cause 3 of the same open messages issue.
AWS: Smithy error FileSystemNotFound, errorKind client, httpError 404; DeleteFileSystem = DELETE /2015-02-01/file-systems/{FileSystemId}. The message is built from the request's FileSystemId: the id appears in single quotes and the sentence is terminated with a period — recorded example "File system 'fs-e220a8397b1dcdaf' does not exist.".
Emulator: the lifecycle transition itself is reached and the status/error class match (404 FileSystemNotFound), but Message is 'File system fs-e220a8397b1dcdaf does not exist' — id unquoted, no terminating period.
Why: suites and clients that extract the quoted id or assert the exact EFS sentence fail against the emulator even though status and code are right, and the divergence appears on every path that resolves a FileSystemId.
Fix: route every EFS file-system lookup through one FileSystemNotFound constructor that quotes the requested id and terminates the sentence.
Traces: efs.delete-file-system-lifecycle-transition.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 2/27 passed, bootstrap_signal=false (the emulator was serving — CreateFileSystem, DescribeFileSystems and DeleteFileSystem all answered). Per-service: efs 2/25 (23 failed). Failures already filed as scaffold-stub issues by the report itself (CreateAccessPoint, DescribeTags, DescribeLifecycleConfiguration, CreateReplicationConfiguration, UpdateFileSystem, and ec2 CreateVpc) are excluded here. The six failures below are the surviving, unowned ones:

1) [mismatch] efs.backup-policy-status-transitions —
   AssertionError: expected PolicyNotFound, got 'NotImplemented'
   (assert initial_code == "PolicyNotFound" on efs.describe_backup_policy(FileSystemId=fs_id) for a file system that never had a backup policy)

2) [mismatch] efs.put-file-system-policy-lockout-rejected —
   AssertionError: a policy denying future PutFileSystemPolicy calls should be rejected as InvalidPolicyException, got 'NotImplemented'
   (assert code == "InvalidPolicyException" on efs.put_file_system_policy with a Deny-PutFileSystemPolicy document and no BypassPolicyLockoutSafetyCheck)

3) [mismatch] efs.update-file-system-throughput-mode-missing-param-rejected —
   AssertionError: expected BadRequest for a provisioned-mode update missing ProvisionedThroughputInMibps, got 'NotImplemented'
   (efs.update_file_system(FileSystemId=fs_id, ThroughputMode="provisioned") with ProvisionedThroughputInMibps omitted)

4) [error] efs.update-file-system-protection-toggle —
   KeyError: 'FileSystemProtection'
   at: initial = efs.describe_file_systems(FileSystemId=fs_id)["FileSystems"][0]["FileSystemProtection"]

5) [mismatch] efs.create-file-system-token-reuse-conflict —
   AssertionError: expected message "File system 'fs-e220a8397b1dcdaf' already exists with creation token 'sim2real-efs-fs-e97ed7afbe3e'", got 'File system already exists with CreationToken sim2real-efs-fs-e97ed7afbe3e' (status 409 and error class FileSystemAlreadyExists matched)

6) [mismatch] efs.delete-file-system-lifecycle-transition —
   AssertionError: expected message "File system 'fs-e220a8397b1dcdaf' does not exist.", got 'File system fs-e220a8397b1dcdaf does not exist' (status 404 and error class FileSystemNotFound matched)

Model consulted: services/service-com-amazonaws-efs-magnolioapiservice-v20150201/smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json — DescribeBackupPolicy (GET /2015-02-01/file-systems/{FileSystemId}/backup-policy, 200, errors incl. PolicyNotFound 404), PutBackupPolicy (PUT same URI, 200, BackupPolicy required), BackupPolicy.Status enum ENABLED|ENABLING|DISABLED|DISABLING, PutFileSystemPolicy (PUT /{FileSystemId}/policy, 200, BypassPolicyLockoutSafetyCheck default false, error InvalidPolicyException client/400), UpdateFileSystem (PUT /{FileSystemId}, 202, error BadRequest client/400), UpdateFileSystemProtection (PUT /{FileSystemId}/protection, 200, output FileSystemProtectionDescription), FileSystemDescription.FileSystemProtection -> FileSystemProtectionDescription{ReplicationOverwriteProtection ENABLED|DISABLED|REPLICATING, "ENABLED by default"}, FileSystemAlreadyExists (client/409, required ErrorCode and FileSystemId), FileSystemNotFound (client/404). Generated file-system ids, creation tokens, hosts and timestamps above are environment-specific examples, not requirements; the statuses, error codes and message construction rules are.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the behavioral suite simparity-behavioral-aws-v1 against it (run_behavioral_suite) and look at the efs service group.
3. Cause 1: run check efs.backup-policy-status-transitions — CreateFileSystem, wait for LifeCycleState=available, then DescribeBackupPolicy; the emulator answers NotImplemented where AWS answers 404 PolicyNotFound (no Message), and PutBackupPolicy ENABLED/DISABLED never drives BackupPolicy.Status.
4. Cause 2: run efs.put-file-system-policy-lockout-rejected — PutFileSystemPolicy with a single Deny statement on elasticfilesystem:PutFileSystemPolicy for Principal * on the file system's own ARN; expect 400 InvalidPolicyException, got NotImplemented. Then DescribeFileSystemPolicy must give 404 PolicyNotFound, and the same policy with BypassPolicyLockoutSafetyCheck=true must succeed and echo the document.
5. Cause 3: run efs.update-file-system-throughput-mode-missing-param-rejected — UpdateFileSystem(FileSystemId, ThroughputMode='provisioned') with ProvisionedThroughputInMibps omitted; expect 400 BadRequest and the file system unchanged at bursting/available, got NotImplemented.
6. Cause 4: run efs.update-file-system-protection-toggle — DescribeFileSystems(FileSystemId=...)['FileSystems'][0] has no FileSystemProtection member (KeyError); then UpdateFileSystemProtection DISABLED/ENABLED must echo the value and be observable on a subsequent DescribeFileSystems.
7. Cause 5: run efs.create-file-system-token-reuse-conflict — CreateFileSystem twice with the same CreationToken; the 409 FileSystemAlreadyExists Message omits the existing file system's id and does not quote the token.
8. Cause 6: run efs.delete-file-system-lifecycle-transition — DeleteFileSystem, then describe the deleted id; the 404 FileSystemNotFound Message does not quote the requested id nor terminate the sentence.
