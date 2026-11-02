Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__kms__s3.refarch-bkp-duplicate-vault-name-rejected-in-region-only
Issue-Key: backup__kms__s3|ListTags|NotImplemented
Issue-Commit: a4ef059b31cd49fc5529baf331c68065b4514ad5

# backup__kms__s3: ListTags answers 501 NotImplemented, so the backup-vault duplicate-name refarch check cannot read back a vault's tags

> _Note: this issue was identified against the code base at commit `a4ef059b31cd49fc5529baf331c68065b4514ad5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTags

## Description
One net-new root cause for the backup__kms__s3 scenario service (1 of 1).

Cause 1 of 1: `ListTags` on a backup vault ARN is refused instead of answered, so no caller can read a vault's tag set back after CreateBackupVault(BackupVaultTags=...).

AWS: Smithy (com.amazonaws.backup#CryoControllerUserManager) models `ListTags` as `GET /tags/{ResourceArn}` with HTTP success code 200, required httpLabel member `ResourceArn` (target `ARN`), optional `NextToken`/`MaxResults` (@range min 1, max 1000) query params, and output `{Tags: map (sensitive), NextToken: string}`. Its only modeled refusals are `InvalidParameterValueException`, `MissingParameterValueException`, `ResourceNotFoundException` (all 400) and `ServiceUnavailableException` (500). For an existing backup vault ARN with tags applied at create time, AWS answers 200 with those tags in `Tags`; there is no modeled 501 and `NotImplemented` is not a member of this operation's error set.

Emulator: the call is refused with HTTP 501 and error code `NotImplemented`, message `operation ListTags is scaffolded but not implemented in this emulator` (fixed text; the variable part is the operation name `ListTags` taken from the request). The handler `services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/list_tags.rs` is still an unmodified scaffold stub (`//! SCAFFOLDED, NOT IMPLEMENTED:` header, returning `Scaffolded::not_implemented(..)`).

Why: a client that creates a vault with `BackupVaultTags` and then verifies, audits or reconciles those tags gets an unmodelled 501/`NotImplemented` its SDK error handling cannot classify (it is not in the operation's modeled error set), instead of a 200 tag map. In this scenario it also hides the thing the check is really asserting — that a rejected duplicate CreateBackupVault must not re-tag the surviving vault — because the tag read needed to prove the absence of that side effect can never be made.

Note on existing issues: the open issue `20261102T110405Z-backup-listtags-stub.md` ("Implement backup ListTags: unimplemented scaffold stub") describes the same stub, but its `Issue-Trace-Ids` carries only the `backup` service check `backup.list-tags-vault-and-plan-round-trip-and-deleted-vault-rejected`. It does not carry the backup__kms__s3 replay key below, so that check stays unverified when that issue is closed. This issue exists to carry that replay key; fixing `list_tags.rs` (real handler reading the service's tag state, then deleting the scaffold header line) closes both.

Fix: the ListTags handler and the service's tag state in `services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/list_tags.rs` — resolve the `ResourceArn` httpLabel against stored backup-vault/plan tag sets and serialize the 200 `{Tags, NextToken}` response; refuse an unknown ARN with the modeled `ResourceNotFoundException` rather than 501.

## Evidence
Check `backup__kms__s3.refarch-bkp-duplicate-vault-name-rejected-in-region-only` (verdict: error).

Expected: after `create_backup_vault(BackupVaultName=<name>, BackupVaultTags={"sim2real": "refarch-bkp"})` and a rejected duplicate create, `list_tags(ResourceArn=<vault arn>)` answers HTTP 200 with `Tags` containing the original create-time tag (i.e. the rejected create applied none of its own tags).

Actual: the call raises
```
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListTags operation: operation ListTags is scaffolded but not implemented in this emulator
```
(HTTP 501). Message breakdown: fixed text `operation ` / ` is scaffolded but not implemented in this emulator`, variable part the operation name from the request. The earlier assertions in the same check (AlreadyExistsException with message "Backup vault with the same name already exists", unchanged CreationDate and BackupVaultArn) were reached and passed; the check dies at the first `ListTags` call.

## Steps to Reproduce
1. Boot the emulator and point a botocore `backup` client at it.
2. `create_backup_vault(BackupVaultName="sim2real-bkp-vault-<suffix>", BackupVaultTags={"sim2real": "refarch-bkp"})`.
3. `describe_backup_vault(BackupVaultName=...)` and take `BackupVaultArn`.
4. `list_tags(ResourceArn=<that arn>)` — observe HTTP 501 / `NotImplemented` instead of a 200 response whose `Tags` map contains `sim2real=refarch-bkp`.
5. Equivalently, replay `backup__kms__s3.refarch-bkp-duplicate-vault-name-rejected-in-region-only`.
