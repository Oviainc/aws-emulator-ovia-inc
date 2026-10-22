Issue-Source: trace-sweep-divergence
Issue-Trace: backup-backup-job-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-backup-job-error-paths backup-backup-job-listing-and-describe-error backup-list-protected-resources-account-wide backup-backup-vault-lifecycle backup-vault-lifecycle-and-listing backup-copy-job-error-paths backup-restore-job-error-paths
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|DescribeBackupJob|4xx->4xx|$.Code
Issue-Key: backup|DescribeBackupJob|4xx->4xx|$.Message
Issue-Key: backup|ListProtectedResources|4xx->4xx|$.Code
Issue-Key: backup|ListProtectedResources|4xx->4xx|$.Message
Issue-Key: backup|CreateBackupVault|4xx->4xx|$.Arn
Issue-Key: backup|CreateBackupVault|4xx->4xx|$.Code
Issue-Key: backup|CreateBackupVault|4xx->4xx|$.CreatorRequestId
Issue-Key: backup|CreateBackupVault|4xx->4xx|$.Message
Issue-Key: backup|ListBackupJobSummaries|4xx->4xx|$.Message
Issue-Key: backup|ListCopyJobSummaries|4xx->4xx|$.Message
Issue-Key: backup|ListRestoreJobSummaries|4xx->4xx|$.Message
Issue-Commit: 7d2cd57c0bb94f32b894957a728316a26e29b449

# Backup error bodies omit the modeled Code and Message members on client-error responses

> _Note: this issue was identified against the code base at commit `7d2cd57c0bb94f32b894957a728316a26e29b449`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeBackupJob

## Description
AWS Backup's client-error structures all carry Code/Message/Type/Context members, and real AWS populates Code (an ERROR_nnnn token) and Message on every one of them. The emulator returns the right exception class and the right HTTP status in these cases but serializes an error body that is missing Code, and in several operations missing Message entirely, so an SDK surfaces an error with no code and sometimes no message at all. This issue bundles SIX causes of that one serialization fault; all six must be closed.

Cause 1 of 6 — DescribeBackupJob, unknown job id.
AWS: Smithy: DescribeBackupJob GET /backup-jobs/{BackupJobId}, error ResourceNotFoundException (httpError 400) whose members are Code, Message, Type, Context. Recorded response: HTTP 400, x-amzn-errortype ResourceNotFoundException, body Code='ERROR_2403', Message='Cannot find Backup job' (a fixed string, not built from the request, so the recorded text is an EXAMPLE; the required part is the presence and value of Code).
Emulator: HTTP 400 ResourceNotFoundException with NO Code member; Message is 'Backup job <id> not found' (example of the divergence: the emulator templates the requested id into a message AWS keeps constant).
Why: a client branching on the Backup error code (ERROR_2403) sees no code at all and cannot distinguish a missing job from any other 400.
Fix: the ResourceNotFoundException construction in the DescribeBackupJob handler / the service's error-to-wire serializer, which must emit Code alongside Message.
Traces: backup-backup-job-error-paths#task#1 (also backup-backup-job-listing-and-describe-error#task#5).

Cause 2 of 6 — ListProtectedResources, out-of-range MaxResults.
AWS: Smithy: ListProtectedResources GET /resources, MaxResults @range min 1 max 1000, error InvalidParameterValueException (httpError 400, members Code/Message/Type/Context). Recorded response: HTTP 400 with Code='ERROR_3607' and Message='Max Results is invalid'.
Emulator: HTTP 400 with BOTH Code and Message absent — an empty error body.
Why: the SDK raises an exception with no code and no message, so a client cannot report or branch on the rejected MaxResults.
Fix: MaxResults range validation in the ListProtectedResources handler plus the shared error serializer.
Traces: backup-list-protected-resources-account-wide#task#2.

Cause 3 of 6 — CreateBackupVault on an existing vault name.
AWS: Smithy: CreateBackupVault PUT /backup-vaults/{BackupVaultName}, error AlreadyExistsException (httpError 400) with members Code, Message, CreatorRequestId, Arn, Type, Context. Recorded response: HTTP 400 with Code='ERROR_1101', Message='Backup vault with the same name already exists', and Arn/CreatorRequestId UNSET (recorded behavior only: AWS does not populate them for this case).
Emulator: HTTP 400 AlreadyExistsException with no Code, with Arn and CreatorRequestId populated from the stored vault, and a message that templates the vault name in ('Backup vault <name> already exists' — an example).
Why: a client keying on ERROR_1101 for duplicate-vault handling gets no code, and the extra Arn/CreatorRequestId members are state AWS does not disclose on this error.
Fix: the AlreadyExistsException path of the CreateBackupVault handler and the error serializer.
Traces: backup-backup-vault-lifecycle#task#4 (also backup-vault-lifecycle-and-listing#task#2).

Cause 4 of 6 — ListBackupJobSummaries, invalid AggregationPeriod.
AWS: Smithy: ListBackupJobSummaries GET /audit/backup-job-summaries, AggregationPeriod targets the AggregationPeriod enum with values ONE_DAY, SEVEN_DAYS, FOURTEEN_DAYS; error InvalidParameterValueException (400). The recorded message is BUILT FROM THE REQUEST and the enum ('Invalid input aggregation id <value>. Input aggregation period must be within ONE_DAY, SEVEN_DAYS, FOURTEEN_DAYS'), so that rule — name the rejected value and list the permitted enum values — is the requirement.
Emulator: HTTP 400 with the Message member absent entirely.
Why: the rejection carries no explanation, so a client cannot tell which parameter was bad or what the legal values are.
Fix: AggregationPeriod enum validation in the ListBackupJobSummaries handler must construct InvalidParameterValueException with the request-derived message.
Traces: backup-backup-job-error-paths#task#2.

Cause 5 of 6 — ListCopyJobSummaries, invalid AggregationPeriod. Same fault and same message rule as cause 4, on the copy-job summaries operation.
AWS: error InvalidParameterValueException (400) with the request-derived 'Invalid input aggregation id ...' message naming the AggregationPeriod enum values.
Emulator: HTTP 400 with Message absent.
Why: as cause 4.
Fix: AggregationPeriod validation in the ListCopyJobSummaries handler.
Traces: backup-copy-job-error-paths#task#2.

Cause 6 of 6 — ListRestoreJobSummaries, invalid AggregationPeriod. Same fault and message rule again.
AWS: error InvalidParameterValueException (400) with the request-derived 'Invalid input aggregation id ...' message.
Emulator: HTTP 400 with Message absent.
Why: as cause 4.
Fix: AggregationPeriod validation in the ListRestoreJobSummaries handler.
Traces: backup-restore-job-error-paths#task#3.

## Evidence
Cause 1 (DescribeBackupJob, backup-backup-job-error-paths#task#1): expected 400 with $.Code='ERROR_2403', $.Message='Cannot find Backup job'; actual 400 with $.Code missing and $.Message='Backup job db89c7cd-... not found'.
Cause 2 (ListProtectedResources, backup-list-protected-resources-account-wide#task#2): expected 400 with $.Code='ERROR_3607', $.Message='Max Results is invalid'; actual 400 with $.Code missing and $.Message missing.
Cause 3 (CreateBackupVault, backup-backup-vault-lifecycle#task#4): expected 400 with $.Code='ERROR_1101', $.Message='Backup vault with the same name already exists', $.Arn and $.CreatorRequestId unset; actual 400 with $.Code missing, $.Arn='arn:aws:backup:...:backup-vault:<name>', $.CreatorRequestId set, $.Message='Backup vault <name> already exists'.
Cause 4 (ListBackupJobSummaries, backup-backup-job-error-paths#task#2): expected 400 with $.Message='Invalid input aggregation id NOT_A_PERIOD. Input aggregation period must be within ONE_DAY, SEVEN_DAYS, FOURTEEN_DAYS'; actual 400 with $.Message missing.
Cause 5 (ListCopyJobSummaries, backup-copy-job-error-paths#task#2): same expected message rule; actual 400 with $.Message missing.
Cause 6 (ListRestoreJobSummaries, backup-restore-job-error-paths#task#3): same expected message rule; actual 400 with $.Message missing.
Smithy citations: ResourceNotFoundException / InvalidParameterValueException / AlreadyExistsException all declare a Code member (AlreadyExistsException additionally Arn and CreatorRequestId); AggregationPeriod enum = [ONE_DAY, SEVEN_DAYS, FOURTEEN_DAYS]; ListProtectedResources.MaxResults @range 1..1000.

## Steps to Reproduce
1. Replay the wire corpus against the emulator (trace set simparity-wire-aws-v1).
2. Cause 1: fixture backup-backup-job-error-paths, step task#1 — GET /backup-jobs/<unknown-id>; inspect the 400 body for Code.
3. Cause 2: fixture backup-list-protected-resources-account-wide, step task#2 — GET /resources?maxResults=<out-of-range>; inspect the 400 body for Code and Message.
4. Cause 3: fixture backup-backup-vault-lifecycle, step task#4 — PUT /backup-vaults/<existing-name>; inspect the 400 body for Code and for the extra Arn/CreatorRequestId members.
5. Causes 4-6: fixtures backup-backup-job-error-paths step task#2, backup-copy-job-error-paths step task#2, backup-restore-job-error-paths step task#3 — GET the respective */summaries route with AggregationPeriod=NOT_A_PERIOD; inspect the 400 body for Message.
