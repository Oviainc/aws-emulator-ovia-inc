Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__kms__secretsmanager.ecsm-rekey-to-aws-managed-key-unlocks-current-version-in-place
Issue-Key: ec2__iam__kms__secretsmanager.ecsm-rekey-to-aws-managed-key-unlocks-current-version-in-place|run#19|AssertionError|assert "alias/aws/secretsmanager" in after[0].get("KmsKeyIds", []), (
Issue-Commit: 755fa21f4b5f658e5c42cafb46f764c1d29f1673

# ec2__iam__kms__secretsmanager: ListSecretVersionIds reports the placeholder 'DefaultEncryptionKey' in a version's KmsKeyIds instead of the KMS key the version is wrapped under

> _Note: this issue was identified against the code base at commit `755fa21f4b5f658e5c42cafb46f764c1d29f1673`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSecretVersionIds

## Description
Cause 1 of 1 — a secret version's KmsKeyIds is a literal placeholder string, not a KMS key reference.
AWS: ListSecretVersionIds answers Versions[] of SecretVersionsListEntry, whose KmsKeyIds member is "The KMS keys used to encrypt the secret version" (Smithy: SecretVersionsListEntry.KmsKeyIds, target KmsKeyIdListType). Every element is therefore a KMS key reference for that version — the same key reference form the secret carries (a key ARN, key id, or the alias when the secret was keyed by alias, including the AWS-managed 'alias/aws/secretsmanager'). Recorded behavior for the scenario that exposes it: UpdateSecret(SecretId, KmsKeyId='alias/aws/secretsmanager') re-wraps the existing AWSCURRENT version in place — ListSecretVersionIds still reports exactly the one pre-existing VersionId — and that entry's KmsKeyIds then names the key the version was re-wrapped under ('alias/aws/secretsmanager'), which is how a caller confirms the re-wrap landed without a new version being minted.
Emulator: the entry is served as {'VersionId': <id>, 'VersionStages': ['AWSCURRENT'], 'CreatedDate': <ts>, 'KmsKeyIds': ['DefaultEncryptionKey']}. 'DefaultEncryptionKey' is not a KMS key reference in any form — not an ARN, key id or alias — and it is reported regardless of the key the secret actually names, so it is a hard-coded placeholder rather than a resolved value. The VersionId and in-place-rewrap parts of the contract are honoured; only KmsKeyIds diverges.
Why: a client cannot learn which KMS key a given secret version is encrypted under. Code that audits per-version key usage, that verifies a rekey (CreateSecret/UpdateSecret with a new KmsKeyId) actually re-wrapped the current version, or that feeds the value back into kms:DescribeKey / a key-policy check, gets a string KMS will reject with a 400 — and an auditor sees every version of every secret on one fictitious key.
Fix: the ListSecretVersionIds handler's SecretVersionsListEntry serializer in the Secrets Manager crate (services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/list_secret_version_ids.rs) must populate KmsKeyIds from the key reference stored on the version (set at CreateSecret/PutSecretValue and updated by UpdateSecret's rekey) instead of a constant; the stored-version state must carry that reference if it does not yet. Related but not covering this: the fixed issue 20261022T235108Z-secrets-manager-listsecretversionids-stub.md stood the operation up — the call is served, not refused, so this is a fidelity gap in its payload, not a regression of that fix.
Traces: ec2__iam__kms__secretsmanager.ecsm-rekey-to-aws-managed-key-unlocks-current-version-in-place.

## Evidence
Expected: after admin.update_secret(SecretId=<secret>, KmsKeyId='alias/aws/secretsmanager'), list_secret_version_ids(SecretId=<secret>)['Versions'] holds the one pre-existing VersionId and that entry's KmsKeyIds contains 'alias/aws/secretsmanager' (the key reference in the form the request gave).
Actual (replay): the entry is {'VersionId': '2f212b1b-6097-4b7c-b9b1-0a0350c1e1dc', 'VersionStages': ['AWSCURRENT'], 'CreatedDate': datetime.datetime(2026, 11, 7, 23, 24, 49, 32629, tzinfo=tzlocal()), 'KmsKeyIds': ['DefaultEncryptionKey']}, and the check fails at `assert "alias/aws/secretsmanager" in after[0].get("KmsKeyIds", [])` with "AssertionError: the re-wrapped version's KmsKeyIds must name the managed key". The VersionId and timestamp shown are this run's generated values; the requirement is only that KmsKeyIds names the key the version is wrapped under. The earlier assertions in the same check passed: DescribeSecret echoed KmsKeyId='alias/aws/secretsmanager' and the rekey minted no new version.

## Steps to Reproduce
1. kms:CreateKey (symmetric) -> <cmk arn>.
2. secretsmanager:CreateSecret(Name=S, SecretString='rekeyed-value', KmsKeyId=<cmk arn>).
3. secretsmanager:ListSecretVersionIds(SecretId=S) — one version; note its KmsKeyIds (already 'DefaultEncryptionKey' rather than the CMK).
4. secretsmanager:UpdateSecret(SecretId=S, KmsKeyId='alias/aws/secretsmanager').
5. secretsmanager:DescribeSecret(SecretId=S) — KmsKeyId is echoed as 'alias/aws/secretsmanager' (correct).
6. secretsmanager:ListSecretVersionIds(SecretId=S) — the same single VersionId is returned (correct), but its KmsKeyIds is ['DefaultEncryptionKey'] instead of containing 'alias/aws/secretsmanager'.
