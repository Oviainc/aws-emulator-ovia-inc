Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__kms__s3.refarch-bkp-e2e-review-backup-plan-cross-region-lifecycle
Issue-Key: backup__kms__s3|ListAliases|NotFoundException
Issue-Commit: 3aa8840434f8d2d1fb0149d64d5521aaa6321d0d

# A backup vault's default EncryptionKeyArn names a KMS key that does not exist, so every KMS read of the vault's key (ListAliases/DescribeKey) answers NotFoundException instead of alias/aws/backup

> _Note: this issue was identified against the code base at commit `3aa8840434f8d2d1fb0149d64d5521aaa6321d0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeBackupVault

## Description
Cause 1 of 1: AWS Backup mints a vault default encryption key inside its OWN state, so the key ARN it publishes is not a key KMS knows about.

Failing check ids: backup__kms__s3.refarch-bkp-e2e-review-backup-plan-cross-region-lifecycle

AWS: `CreateBackupVault` models `EncryptionKeyArn` (Smithy: backup#CreateBackupVault input member `EncryptionKeyArn`, target `ARN`, optional) and documents that "if you do not specify a key, Backup creates a KMS key for you by default". The key a vault gets in that case is the AWS-managed Backup key, whose AWS alias is `alias/aws/backup` — KMS documents the form of these names on ListAliases: "You can recognize Amazon Web Services aliases because their names have the format `aws/<service-name>`, such as `aws/dynamodb`" (Smithy: kms#ListAliases doc). Because the key really exists in KMS, `DescribeBackupVault` (Smithy: backup#DescribeBackupVault output member `EncryptionKeyArn`, target `ARN`) returns an ARN that resolves in KMS: `kms:ListAliases` with `KeyId` set to that ARN (Smithy: kms#ListAliases input `KeyId`, `@length 1..2048`) returns a non-empty `Aliases` list whose first `AliasName` is `alias/aws/backup`, and `kms:DescribeKey` on it reports `KeyManager=AWS`. KMS's modeled `NotFoundException` (client, HTTP 404) is reserved for a key id or ARN that names NO existing key.

Emulator: a vault created with no `EncryptionKeyArn` is given an ARN synthesised entirely inside the Backup crate's own store — `ComAmazonawsBackupCryocontrollerusermanagerState::default_encryption_key_arn` mints `arn:aws:kms:<region>:<account>:key/<backup-crate sequence id>` and records it in `default_encryption_keys`. No key and no alias is ever created in the KMS emulator's key store for it. `DescribeBackupVault` then happily reports that ARN, but the very next cross-service call on it is refused: `kms:ListAliases(KeyId=<vault EncryptionKeyArn>)` answers `NotFoundException` with message `Key '<that ARN>' does not exist` instead of returning `[{AliasName: "alias/aws/backup", ...}]`. In the failing replay the refused ARN was of the form `arn:aws:kms:<region>:<account>:key/<uuid>` — the ARN value itself is a per-run generated id and is NOT part of the requirement; what is required is that whatever ARN the vault publishes resolves in KMS. The message's fixed text is `Key '` ... `' does not exist`; its only variable part is the key id/ARN taken verbatim from the request.

Why: a client cannot follow the vault back to its key. Any workflow that reads `DescribeBackupVault.EncryptionKeyArn` and then asks KMS about that key — to confirm the vault is on the AWS-managed Backup key rather than a CMK, to read its aliases, to check rotation, to attach a policy, or to assert `EncryptionKeyType` — gets a 404 for a key the Backup service just told it exists. The two services disagree about the same resource, so the whole "which key protects this vault" review path (the frozen trajectory backup.DescribeBackupVault -> kms.ListAliases) is unrunnable, and a client that treats KMS NotFoundException as "the key was deleted" concludes the vault is broken.

Fix: the Backup vault state/create path. `default_encryption_key_arn` in services/service-com-amazonaws-backup-cryocontrollerusermanager/src/state.rs must stop minting a private KMS ARN in `default_encryption_keys` and instead resolve (provisioning on first use) the AWS-managed key behind the predefined alias `alias/aws/backup` in the shared KMS store, recording THAT key's ARN on the vault; `crate::ops::create_backup_vault` is the caller. The KMS side already has the machinery for a predefined `alias/aws/<service-name>` key (an AWS-managed `Cmk` plus its `Alias`), so this is a wiring gap in Backup, not a missing KMS capability. `EncryptionKeyType` should then be reported consistently with the key actually resolved.

Traces: backup__kms__s3.refarch-bkp-e2e-review-backup-plan-cross-region-lifecycle — fails in phase 2/4 (solve) at the `kms.ListAliases` step of the frozen trajectory, with `NotFoundException: An error occurred (NotFoundException) when calling the ListAliases operation: Key '<vault EncryptionKeyArn>' does not exist`, before the check can assert `vault_key_alias == "alias/aws/backup"`. Relevant code: services/service-com-amazonaws-backup-cryocontrollerusermanager/src/state.rs (default_encryption_key_arn, ~line 1169; `default_encryption_keys`, ~line 997; `kms_key_arn`, line 239) and services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/create_backup_vault.rs lines 252-260; the KMS AWS-managed-key path is services/service-com-amazonaws-kms-trentservice/src/state.rs (~line 728, `Cmk::aws_managed` + `insert_alias`, and `is_predefined_aws_alias`, ~line 1163).

## Evidence
Expected (AWS): DescribeBackupVault on a vault created without EncryptionKeyArn returns an EncryptionKeyArn for the AWS-managed Backup key; kms:ListAliases(KeyId=<that ARN>) then returns HTTP 200 with Aliases[0].AliasName == "alias/aws/backup".

Actual (emulator): DescribeBackupVault returns a KMS key ARN minted inside the Backup crate's own state (arn:aws:kms:<region>:<account>:key/<generated id>; the observed run showed arn:aws:kms:us-west-2:000000000000:key/18e27e1a-e32d-92ef-0000-000000000001 — a per-run generated id, not part of the requirement). kms:ListAliases(KeyId=<that ARN>) is refused:
  HTTP 404, Error.Code = NotFoundException
  Error.Message = "Key '<the ARN from the request>' does not exist"
(fixed text: "Key '" ... "' does not exist"; variable part: the key ARN echoed from the request).

The check never reaches its assertion `answer["vault_key_alias"] == "alias/aws/backup"`; the suite reports:
_PhaseError: [phase 2/4 solve] NotFoundException: An error occurred (NotFoundException) when calling the ListAliases operation: Key 'arn:aws:kms:us-west-2:000000000000:key/18e27e1a-e32d-92ef-0000-000000000001' does not exist

## Steps to Reproduce
1. backup:CreateBackupVault with a BackupVaultName and NO EncryptionKeyArn (the AWS::Backup::BackupVault resource in the check's CloudFormation template does exactly this).
2. backup:DescribeBackupVault with that BackupVaultName; note the returned EncryptionKeyArn.
3. kms:ListAliases with KeyId set verbatim to that EncryptionKeyArn.
   Expected: HTTP 200, Aliases[0].AliasName == "alias/aws/backup".
   Actual: HTTP 404 NotFoundException, "Key '<that ARN>' does not exist".
4. kms:DescribeKey with the same KeyId shows the same divergence (the key is unknown to KMS).

Or replay the behavioral check directly: backup__kms__s3.refarch-bkp-e2e-review-backup-plan-cross-region-lifecycle.
