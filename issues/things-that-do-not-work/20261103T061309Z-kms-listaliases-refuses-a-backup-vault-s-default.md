Issue-Source: trace-sweep-divergence
Issue-Trace: backup-rbx-traj-reference-read-chain-tag-selection-and-managed-key
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-rbx-traj-reference-read-chain-tag-selection-and-managed-key
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: kms|ListAliases|2xx->4xx|$.AliasCount
Issue-Key: kms|ListAliases|2xx->4xx|$.Aliases
Issue-Key: kms|ListAliases|2xx->4xx|$.Truncated
Issue-Key: kms|ListAliases|2xx->4xx|status
Issue-Commit: 2b752dc009b2ad79b0f982086ab178b11cbad40c

# KMS ListAliases refuses a backup vault's default managed key with 400 instead of returning the alias/aws/backup alias list

> _Note: this issue was identified against the code base at commit `2b752dc009b2ad79b0f982086ab178b11cbad40c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAliases

## Description
One net-new wire divergence: a kms:ListAliases read of the KMS key that an AWS Backup vault reports as its default EncryptionKeyArn is refused with HTTP 400 instead of returning that key's aliases. The open issues 20261027T205606Z-backup-kms-s3-a-backup-vault-s-default-encryptionkeyarn-names-a.md and 20261027T205645Z-backup-a-backup-vault-s-default-encryptionkeyarn-names-a.md describe the same family of defect but carry neither this fixture nor this step as a replay key, so this cluster is filed here with its own linkage.

Cause 1 of 1 - ListAliases on a backup vault's default key answers 400 instead of 200 (fixture backup-rbx-traj-reference-read-chain-tag-selection-and-managed-key, step task#9).
AWS: Smithy kms#ListAliases takes an optional KeyId (length 1-2048, a key id or ARN) and returns 200 with Aliases (kms#AliasList of {AliasArn, AliasName, CreationDate, LastUpdatedDate, TargetKeyId}), Truncated (modelled default false) and, when paginated, NextMarker. The key a Backup vault names in EncryptionKeyArn is the AWS managed key for Backup, so the response carries exactly one alias whose AliasName is 'alias/aws/backup' and whose AliasArn is 'arn:aws:kms:<region>:<account>:alias/aws/backup', with TargetKeyId the key's id and Truncated false; the recording also shows AliasCount reported alongside. The key id, account, region and the two dates are per-environment values; the alias name and the 200-with-one-alias shape are the requirement. The only not-found class ListAliases models is NotFoundException at HTTP 404, so a 400 is not a modelled outcome of naming an existing key.
Emulator: HTTP 400 - the whole body is absent (Aliases, AliasCount and Truncated all missing), i.e. the key the Backup emulator advertised as the vault's EncryptionKeyArn is not resolvable in the KMS emulator's state.
Why: any client that reads a vault and then follows its EncryptionKeyArn into KMS (console-style reference chains, compliance/inventory tooling, encryption-key validation before a backup job) fails on a request AWS answers successfully, and it fails with the wrong class as well: a 400 is a malformed-request signal, so retry/fallback logic never reaches its not-found branch.
Fix: KMS alias/key state must contain the AWS managed key and 'alias/aws/backup' alias that the backup crate hands out as a vault's default EncryptionKeyArn (seed it when the vault's default key is minted), and ListAliases must resolve that KeyId instead of refusing 400.
Traces: backup-rbx-traj-reference-read-chain-tag-selection-and-managed-key#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up), cluster service=kms|ListAliases|status=200->400:
fixture backup-rbx-traj-reference-read-chain-tag-selection-and-managed-key, step task#9 - status: expected 200, got 400; $.Aliases missing (expected [{'AliasArn': 'arn:aws:kms:us-west-2:000000000000:alias/aws/backup', 'AliasName': 'alias/aws/backup', 'CreationDate': <timestamp>, 'LastUpdatedDate': <timestamp>, 'TargetKeyId': <key id>}], got None); $.AliasCount missing (expected 1, got None); $.Truncated missing (expected False, got None).
Model checked: kms#ListAliases input (KeyId, Limit, Marker), output (Aliases, NextMarker, Truncated default false), error set - NotFoundException is 404, InvalidArnException/InvalidMarkerException are the only 400 classes.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus, trace set simparity-wire-aws-v1).
2. Replay fixture backup-rbx-traj-reference-read-chain-tag-selection-and-managed-key: create a backup vault, read it with DescribeBackupVault and take its EncryptionKeyArn.
3. Step task#9: call kms:ListAliases with KeyId set to that EncryptionKeyArn - the emulator answers HTTP 400 with no body where AWS answers 200 with the single 'alias/aws/backup' alias and Truncated false.
