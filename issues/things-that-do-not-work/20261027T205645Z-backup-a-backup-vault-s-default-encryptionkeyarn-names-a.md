Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__kms__s3.refarch-bkp-e2e-review-backup-plan-cross-region-lifecycle
Issue-Key: backup__kms__s3|ListAliases|NotFoundException
Issue-Commit: f7e63039319d8a0111addfa90cab83fc14fd19d7

# A backup vault's default EncryptionKeyArn names a KMS key that does not exist, so kms:ListAliases on it is refused NotFoundException instead of resolving alias/aws/backup (1 cause)

> _Note: this issue was identified against the code base at commit `f7e63039319d8a0111addfa90cab83fc14fd19d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeBackupVault

## Description
One net-new root cause, grounded in the failing behavioral check backup__kms__s3.refarch-bkp-e2e-review-backup-plan-cross-region-lifecycle (phase 2/4 'solve'). The check id's leading token is the composite group `backup__kms__s3`, which is not a service; the defect is attributed to `backup` because AWS Backup is the service that mints and reports the vault's encryption-key reference. (All three constituent services — backup, kms, s3 — are stood up and answering.)

Cause 1 of 1: a backup vault created without an explicit EncryptionKeyArn is given a synthetic KMS key ARN that the emulator's KMS has never heard of, so every cross-service read of that ARN is refused.

AWS: CreateBackupVault's EncryptionKeyArn is optional (Smithy: backup CreateBackupVault input member EncryptionKeyArn, target ARN, not required; DescribeBackupVault output carries EncryptionKeyArn: ARN and EncryptionKeyType). When it is omitted, AWS Backup encrypts the vault with the account's AWS-managed Backup key, and DescribeBackupVault returns THAT key's ARN — a key that really exists in KMS in the same account and region, whose alias is `alias/aws/backup`. Consequently kms:ListAliases(KeyId=<that ARN>) is a 200 whose Aliases list contains the entry with AliasName `alias/aws/backup` (Smithy: kms ListAliases input KeyId, output Aliases/NextMarker/Truncated; its modeled refusals are InvalidArnException, InvalidMarkerException, NotFoundException — NotFoundException is for a key that does not exist, which an AWS-managed key handed out by Backup never is). The check encodes exactly this contract: `aliases[0] == 'alias/aws/backup'` for the ARN DescribeBackupVault reported.

Emulator: DescribeBackupVault returns an EncryptionKeyArn of the shape `arn:aws:kms:<region>:<account>:key/<synthetic uuid>` (the recorded run shows a counter-shaped id ending `-0000-000000000001`) for which no key exists in the KMS emulator's key store, and no `alias/aws/*` alias exists either. Passing that ARN straight back to kms:ListAliases is refused: error code `NotFoundException`, HTTP 404, message `Key '<key-arn>' does not exist` — fixed text `Key '` ... `' does not exist`, with the ARN taken verbatim from the request's KeyId. The refusal aborts the check in phase 2 of 4, so its cleanup and destroy phases (versioned-bucket emptying, stack delete, the deleted-vault DescribeBackupVault refusal contract) were never exercised at all.

Why: a client cannot do the single most common Backup audit step — resolve the vault's encryption key to a human-readable alias to tell an AWS-managed vault from a CMK-backed one — because the ARN Backup hands out is a dangling reference. Any tooling that round-trips DescribeBackupVault.EncryptionKeyArn into KMS (ListAliases, DescribeKey, GetKeyPolicy, key-state checks) gets a 404 for a key the emulator itself just advertised, and compliance/review flows that branch on `alias/aws/backup` vs a customer alias cannot run.

Fix: the backup vault's state/creation path — CreateBackupVault's default-encryption handling must resolve (and, on demand, materialise) the account's AWS-managed Backup key in the KMS emulator's key store, aliased `alias/aws/backup`, and store THAT key's ARN as the vault's EncryptionKeyArn, rather than fabricating an ARN in Backup's own handler; DescribeBackupVault then echoes a reference KMS can resolve. This is adjacent to, but not covered by, open issue 20261024T102105Z-kms-no-authorization-is-evaluated-putkeypolicy-has-no.md (its Cause 3, 'the AWS-managed key catalogue does not exist', is grounded in kms.aws-managed-key-reference-catalogue and only describes ListAliases returning zero aliases on a fresh account); even with that catalogue seeded, Backup would still be pointing at a key id it invented, so this cause needs its own fix on the Backup side. No cause here is marked ALREADY FILED or OWNED in the replay report.

Traces: backup__kms__s3.refarch-bkp-e2e-review-backup-plan-cross-region-lifecycle.

## Evidence
backup__kms__s3.refarch-bkp-e2e-review-backup-plan-cross-region-lifecycle — [error] _PhaseError: [phase 2/4 solve] NotFoundException: An error occurred (NotFoundException) when calling the ListAliases operation: Key 'arn:aws:kms:us-west-2:000000000000:key/18e27e3b-b617-0a0c-0000-000000000001' does not exist.

The failing call is the frozen solve trajectory's 5th step: kms.ListAliases(KeyId=DescribeBackupVault(BackupVaultName=<stack's vault>)['EncryptionKeyArn']). The four preceding steps (backup.GetBackupPlan, backup.ListBackupSelections, backup.GetBackupSelection, backup.DescribeBackupVault) all succeeded, so the vault DID report an EncryptionKeyArn — it just does not resolve in KMS.

Expected (AWS): HTTP 200 from ListAliases, Aliases[0].AliasName == 'alias/aws/backup' (the check asserts: "the vault's key must resolve in KMS to the AWS-managed Backup alias").
Actual (emulator): HTTP 404, Error.Code 'NotFoundException', Error.Message "Key '<the ARN just returned by DescribeBackupVault>' does not exist". The ARN value shown above is one run's generated id and is not part of the requirement; the requirement is that whatever ARN DescribeBackupVault reports resolves in KMS and carries alias/aws/backup.

Collateral: because the phase aborts, phases 3 (cleanup) and 4 (destroy) of this check are never reached — DEPTH_REACHED=2/4 — so the emulator's behavior for emptying the versioned backup bucket, stack deletion, and the deleted-vault DescribeBackupVault refusal is unmeasured by this run.

## Steps to Reproduce
1. Against the emulator, create a backup vault with no EncryptionKeyArn: `aws backup create-backup-vault --backup-vault-name my-vault` (the check reaches it via a CloudFormation stack that declares an AWS::Backup::BackupVault, but a direct call reproduces it).
2. Read the vault back: `aws backup describe-backup-vault --backup-vault-name my-vault --query EncryptionKeyArn` — note the returned key ARN.
3. Hand that exact ARN to KMS: `aws kms list-aliases --key-id <EncryptionKeyArn from step 2>`.
   Observed: HTTP 404 NotFoundException, "Key '<arn>' does not exist".
   Expected (AWS): HTTP 200 with an Aliases entry whose AliasName is 'alias/aws/backup'.
4. Same for `aws kms describe-key --key-id <that ARN>` — the advertised key is unknown to KMS.
5. Re-replay check backup__kms__s3.refarch-bkp-e2e-review-backup-plan-cross-region-lifecycle to confirm the fix; it must now get past phase 2/4 'solve'.
