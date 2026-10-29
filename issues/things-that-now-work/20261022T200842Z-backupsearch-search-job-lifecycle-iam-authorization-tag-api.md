Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backupsearch.start-search-job-fails-with-empty-scope backupsearch.start-search-job-nonexistent-backup-resource-arn-accepted-then-fails-async backupsearch.list-search-job-results-and-backups-structure-for-completed-job backupsearch.e2e-search-job-tag-lifecycle backupsearch.stop-search-job-idempotent-when-already-stopped backupsearch.tag-resource-not-found-with-positive-control backupsearch.list-tags-for-resource-not-found backupsearch.access-denied-without-backupsearch-permission-rejected backupsearch.start-search-job-invalid-encryption-key-arn-rejected
Issue-Shape-Id: com.amazonaws.backupsearch#CryoBackupSearchService
Issue-Key: backupsearch.start-search-job-fails-with-empty-scope|run#8|AssertionError|assert final["Status"] == "FAILED", f"expected empty scope to fail, got: {final!r}"
Issue-Key: backupsearch.start-search-job-nonexistent-backup-resource-arn-accepted-then-fails-async|run#10|AssertionError|assert final["Status"] == "FAILED", (
Issue-Key: backupsearch.list-search-job-results-and-backups-structure-for-completed-job|run#14|AssertionError|assert final["Status"] == "FAILED", f"expected empty-scope refusal, got: {final!r}"
Issue-Key: backupsearch.e2e-search-job-tag-lifecycle|run#13|AssertionError|assert final["Status"] == "FAILED", f"expected empty-scope job to fail, got {final!r}"
Issue-Key: backupsearch.stop-search-job-idempotent-when-already-stopped|run#9|AssertionError|assert code is None, f"expected idempotent success, got error code={code}"
Issue-Key: backupsearch.tag-resource-not-found-with-positive-control|run#8|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got code={code!r}"
Issue-Key: backupsearch.list-tags-for-resource-not-found|run#9|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got code={code!r}"
Issue-Key: backupsearch.access-denied-without-backupsearch-permission-rejected|_error#14|AssertionError|raise AssertionError(f"expected rejection, but call succeeded and returned {result!r}")
Issue-Key: backupsearch.start-search-job-invalid-encryption-key-arn-rejected|_client_error#2|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Commit: dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c

# BackupSearch search-job lifecycle, IAM authorization, tag-API not-found code and EncryptionKeyArn validation diverge from AWS (5 causes)

> _Note: this issue was identified against the code base at commit `dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSearchJob

## Description
Five distinct root causes in the BackupSearch (sdkId BackupSearch, restJson1) emulator, all reached through the search-job plane. Each cause must be closed before this issue is done.

=== Cause 1 of 5: a search job whose scope resolves to nothing settles to COMPLETED where AWS settles it to FAILED ===
AWS: recorded behavior only — the Smithy model's SearchJobState enum declares RUNNING/STOPPING/STOPPED/COMPLETED/FAILED but documents no transition rule. The recordings show that a StartSearchJob whose SearchScope selects no recovery points (BackupResourceTypes=["S3"] with no matching backups, or a SourceResourceArns/BackupResourceArns list naming resources that hold no recovery point) is ACCEPTED (200) and then resolves asynchronously to Status=FAILED, not COMPLETED. The recordings further show the follow-on rule that StopSearchJob on such a FAILED job is a 409 ConflictException (example message: "SearchJob can not be stopped since it is in FAILED state. Please try again later").
Emulator: every job it mints is owed COMPLETED unconditionally — start_search_job.rs sets next_status_due = (now + TRANSITION_DELAY, COMPLETED) regardless of scope — so GetSearchJob reports Status=COMPLETED with SearchScopeSummary all-zero, and StopSearchJob on it reports the COMPLETED conflict wording instead of the FAILED one.
Why: a client that waits for a terminal status and branches on it (retry/alert on FAILED, read results on COMPLETED) takes the success branch for a search that in fact found nothing and failed, and any state machine gated on FAILED never fires.
Fix: the search-job settle path and the status the StartSearchJob handler owes a job (the service's state module plus the StartSearchJob handler): decide the owed terminal status from the resolved scope, FAILED when nothing is in scope.
Traces: backupsearch.start-search-job-fails-with-empty-scope, backupsearch.start-search-job-nonexistent-backup-resource-arn-accepted-then-fails-async, backupsearch.list-search-job-results-and-backups-structure-for-completed-job, backupsearch.e2e-search-job-tag-lifecycle; code at services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/start_search_job.rs-equivalent src/ops/start_search_job.rs:269-288 and src/state.rs:40-53. NOTE: backupsearch.e2e-search-job-tag-lifecycle additionally calls UntagResource, which is an unimplemented scaffold stub already filed separately; that check cannot go green until that issue also lands.

=== Cause 2 of 5: StopSearchJob is not idempotent on an already-STOPPED job ===
AWS: recorded behavior only — a second StopSearchJob on a job that already reached STOPPED answers HTTP 200 with an empty body and leaves Status=STOPPED. (The model marks StopSearchJob @idempotent and its prose says only RUNNING can be stopped, but the recording shows the already-STOPPED repeat is a no-op success, not a conflict.)
Emulator: src/ops/stop_search_job.rs:178-188 treats every terminal status, STOPPED included, as a conflict and answers 409 ConflictException.
Why: a client or SDK retry that re-sends the stop (the operation is modelled @idempotent, so retries are expected) gets a hard 409 on a job that is already in the state it asked for, turning a safe retry into a caller-visible failure.
Fix: the StopSearchJob handler's status gate — STOPPED (and STOPPING) must short-circuit to an empty 200; only COMPLETED and FAILED conflict.
Traces: backupsearch.stop-search-job-idempotent-when-already-stopped; src/ops/stop_search_job.rs:178-194.

=== Cause 3 of 5: the tag APIs answer the wrong error code for a nonexistent resource ARN ===
AWS: recorded behavior only, and it deliberately diverges from the model — Smithy declares ResourceNotFoundException (HTTP 404) on both TagResource and ListTagsForResource, but the recorded responses for a syntactically well-formed search-job ARN that names no existing job carry error code "NotFoundException" (example message: "Resource was not found"). The code is the requirement; the message text is an example.
Emulator: both operations answer code ResourceNotFoundException (the modelled shape).
Why: client code and SDK error-matching branch on the error code string; a caller that catches NotFoundException around TagResource/ListTagsForResource (as it must against real AWS) does not catch the emulator's ResourceNotFoundException and the error escapes as an unhandled fault.
Fix: the error channel of the TagResource and ListTagsForResource handlers — serialize the not-found failure of these two tag operations with the "NotFoundException" code AWS actually returns.
Traces: backupsearch.tag-resource-not-found-with-positive-control, backupsearch.list-tags-for-resource-not-found; services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/ops/tag_resource.rs and .../list_tags_for_resource.rs.

=== Cause 4 of 5: no IAM authorization — StartSearchJob succeeds for a principal with zero permissions ===
AWS: Smithy declares AccessDeniedException (httpError 403, errorKind client) as a service-common error on every BackupSearch operation. A caller using credentials from an assumed role with no attached identity-based policy is rejected 403 AccessDeniedException, with AWS's standard authorization-failure message built from the request — "User: <assumed-role principal ARN> is not authorized to perform: backup-search:StartSearchJob on resource: arn:aws:backup-search:<region>:<account>:/search-jobs because no identity-based policy allows the backup-search:StartSearchJob action" — and no search job is created.
Emulator: the call returns HTTP 200 and mints a job (SearchJobArn arn:aws:backup-search:<region>:<account>:search-job/<id>), i.e. authorization is not evaluated at all for this service.
Why: this is an overly-permissive emulator. Negative-path tests that assert a deny are silently inverted, and callers cannot develop or verify least-privilege IAM policies against the emulator because every unauthorized call succeeds and leaves a real resource behind.
Fix: request authorization for this service's operations (the signed principal's permissions must be evaluated before the handler runs, and the denial serialized as the service's AccessDeniedException), ahead of the StartSearchJob handler.
Traces: backupsearch.access-denied-without-backupsearch-permission-rejected.

=== Cause 5 of 5: StartSearchJob does not validate/resolve EncryptionKeyArn ===
AWS: Smithy StartSearchJobInput.EncryptionKeyArn (shape EncryptionKeyArn) with ResourceNotFoundException (HTTP 404) declared on StartSearchJob. The recording shows that an EncryptionKeyArn which cannot be resolved to a KMS key is rejected with ResourceNotFoundException and no job is created (the message relays KMS's own text and embeds a per-call request id, so the message is NOT a requirement here — only the 404/ResourceNotFoundException is).
Emulator: the malformed/unresolvable EncryptionKeyArn is accepted, the call answers HTTP 200 and a search job is created and persisted.
Why: a caller that mistypes or passes a key ARN from another account/region gets a job that appears healthy instead of the 404 AWS raises, so the failure surfaces later (or never) and encryption configuration errors go undetected.
Fix: the StartSearchJob handler's input validation — resolve EncryptionKeyArn before minting the job and answer the modelled ResourceNotFoundException when it names no key.
Traces: backupsearch.start-search-job-invalid-encryption-key-arn-rejected; services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/ops/start_search_job.rs:265-289 (EncryptionKeyArn is stored, never checked).

## Evidence
Cause 1 — expected GetSearchJob Status="FAILED" for an unmatched scope; got HTTP 200 with Status="COMPLETED", SearchScopeSummary {TotalRecoveryPointsToScanCount: 0, TotalItemsToScanCount: 0}, CompletionTime set (replay: "AssertionError: expected empty scope to fail, got: {... 'Status': 'COMPLETED' ...}" and "expected the bogus-ARN job to resolve to FAILED (empty scope), got: {... 'Status': 'COMPLETED' ...}").
Cause 2 — expected second StopSearchJob on a STOPPED job to return HTTP 200 (no error); got error code ConflictException (replay: "AssertionError: expected idempotent success, got error code=ConflictException").
Cause 3 — expected error code "NotFoundException" from TagResource and ListTagsForResource on a nonexistent search-job ARN; got "ResourceNotFoundException" (replay: "AssertionError: expected NotFoundException, got code='ResourceNotFoundException'" in both checks).
Cause 4 — expected 403 AccessDeniedException from StartSearchJob for an assumed role with no attached policy; got HTTP 200 and a created job: {'ResponseMetadata': {'HTTPStatusCode': 200...}, 'SearchJobArn': 'arn:aws:backup-search:us-west-2:000000000000:search-job/searchjob-000000000001', 'SearchJobIdentifier': 'searchjob-000000000001'} (replay: "AssertionError: expected rejection, but call succeeded and returned {...}").
Cause 5 — expected 404 ResourceNotFoundException from StartSearchJob with an unresolvable EncryptionKeyArn; got HTTP 200 and a created job (replay: "AssertionError: expected a ClientError, but the call succeeded and returned {... 'SearchJobIdentifier': 'searchjob-000000000001'}").

## Steps to Reproduce
Boot the emulator and point a botocore backupsearch client at it, then:
1. (Cause 1) start_search_job(Name=..., SearchScope={"BackupResourceTypes": ["S3"]}); poll get_search_job until Status is terminal — emulator reports COMPLETED, AWS reports FAILED. Repeat with SearchScope={"BackupResourceTypes":["S3"],"BackupResourceArns":["arn:aws:backup:<region>:<acct>:recovery-point:00000000-0000-0000-0000-000000000000"]} for the same divergence. Then stop_search_job on the terminal job and compare the ConflictException wording (AWS names FAILED).
2. (Cause 2) start_search_job(...); stop_search_job(...); poll get_search_job until Status=="STOPPED"; call stop_search_job again — emulator raises 409 ConflictException, AWS returns 200 and leaves Status=STOPPED.
3. (Cause 3) start_search_job(...), take SearchJobArn, replace the id segment with 00000000-0000-0000-0000-000000000000, then tag_resource(ResourceArn=<bogus>, Tags={"k":"v"}) and list_tags_for_resource(ResourceArn=<bogus>) — inspect err.response["Error"]["Code"]: emulator ResourceNotFoundException, AWS NotFoundException.
4. (Cause 4) iam.create_role with a trust policy for the account root and NO attached policies; sts.assume_role; build a backupsearch client from those credentials; start_search_job(Name=..., SearchScope={"BackupResourceTypes":["S3"]}) — emulator returns 200 and creates a job, AWS returns 403 AccessDeniedException and creates nothing.
5. (Cause 5) start_search_job(Name=..., SearchScope={"BackupResourceTypes":["S3"]}, EncryptionKeyArn=<well-formed but unresolvable key ARN>) — emulator returns 200, AWS raises ResourceNotFoundException.
