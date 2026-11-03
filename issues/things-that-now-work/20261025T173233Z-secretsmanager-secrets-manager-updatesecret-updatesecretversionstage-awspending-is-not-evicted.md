Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.update-secret-evicts-a-separately-staged-awspending-label secretsmanager.update-secret-kms-key-change-re-encrypts-so-the-old-key-is-no-longer-needed secretsmanager.update-secret-version-stage-remove-current-without-move-fails
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager.update-secret-evicts-a-separately-staged-awspending-label|run#18|AssertionError|assert v2 not in after, (
Issue-Key: secretsmanager.update-secret-kms-key-change-re-encrypts-so-the-old-key-is-no-longer-needed|run#20|Timeout|time.sleep(3.0)
Issue-Key: secretsmanager.update-secret-version-stage-remove-current-without-move-fails|_error#10|AssertionError|raise AssertionError("expected the call to be rejected, but it succeeded")
Issue-Commit: e730bd97e23027ce38421dab7cedb53db537ccde

# Secrets Manager UpdateSecret/UpdateSecretVersionStage: AWSPENDING is not evicted on a new current version, a KmsKeyId change is metadata-only (KMS never consulted), and AWSCURRENT can be removed outright (3 causes)

> _Note: this issue was identified against the code base at commit `e730bd97e23027ce38421dab7cedb53db537ccde`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateSecret

## Description
Three independent root causes across UpdateSecret and UpdateSecretVersionStage, each grounded in a failing behavioral check of the secretsmanager suite and marked neither "ALREADY FILED" nor "OWNED by". Both operations answer (they are no longer 501 scaffolds), so these are fidelity divergences in implemented handlers. Related but NOT covering: 20261023T182429Z-secretsmanager-secrets-manager-version-history-is-not-tracked-staging.md (version history/staging labels are now tracked — the surviving defect below is the specific eviction rule) and 20261024T190718Z-secretsmanager-secrets-manager-enforces-no-authorization-and-never-consults.md (names the absence of KMS consultation for other operations, not for an UpdateSecret KmsKeyId change).

=== Cause 1 of 3: a new AWSCURRENT does not evict a separately staged AWSPENDING label ===
AWS: Smithy: UpdateSecret — "If you include SecretString or SecretBinary to create a new secret version, Secrets Manager automatically moves the staging label AWSCURRENT to the new version. Then it attaches the label AWSPREVIOUS to the version that AWSCURRENT was removed from"; UpdateSecretVersionStage — "Each staging label can be attached to only one version at a time" and "If this action results in the last label being removed from a version, then the version is considered to be 'deprecated'". Recorded behavior on real AWS adds the rule the model does not spell out: a write that mints a new AWSCURRENT without re-naming AWSPENDING also EVICTS AWSPENDING from whichever version a prior PutSecretValue staged it on — the evicted version afterwards carries NO staging labels at all, while its bytes stay readable by VersionId. The same eviction holds for a plain PutSecretValue that omits VersionStages; minting a new current version is what cancels a stale pending version.
Emulator: the prior version keeps AWSPENDING. Replay: after UpdateSecret, the separately staged version's staging list is ['AWSPENDING'] where AWS leaves no entry in the staging index at all.
Why: a client that treats "a version staged AWSPENDING exists" as "a rotation is in flight" (which is exactly how the Secrets Manager rotation protocol and rotation Lambdas decide what to do) sees a phantom in-flight rotation forever after any unrelated write. Rotation drivers either refuse to start or try to finish a rotation whose pending version real AWS already cancelled.
Fix: the staging-index update in the UpdateSecret handler's value-write path and the shared version-write helper it calls — services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/update_secret.rs (the `stores_a_value && !replay` branch and `put_version`), which must also detach AWSPENDING from any other version when it attaches a new AWSCURRENT.
Traces: secretsmanager.update-secret-evicts-a-separately-staged-awspending-label.

=== Cause 2 of 3: UpdateSecret(KmsKeyId=...) only rewrites metadata, and GetSecretValue never consults the secret's CMK ===
AWS: Smithy: UpdateSecretRequest$KmsKeyId, with UpdateSecret modeling both DecryptionFailure and EncryptionFailure (HTTP 400, "Secrets Manager can't decrypt/encrypt the protected secret text using the provided KMS key"), and GetSecretValue modeling DecryptionFailure. Changing only KmsKeyId moves the stored secret to the new key: DescribeSecret then names the new key, the OLD key can be disabled and GetSecretValue still returns the original value, and the change creates no AWSPENDING version (GetSecretValue(VersionStage="AWSPENDING") must refuse ResourceNotFoundException with "Secrets Manager can't find the specified secret value for staging label: AWSPENDING" — fixed text with the staging label taken from the request). Conversely, once the NEW key is disabled, GetSecretValue must fail DecryptionFailure (the message embeds the downstream KMS request id and is per-run, so only the code is a requirement).
Emulator: the handler stores `secret.kms_key_id` and nothing else — no KMS call is made on write or on read. The secret's plaintext is served regardless of the state of either key, so the DecryptionFailure the check waits for after disabling the new key never arrives and the check is killed at the harness cap: "Check timed out after 60.0s". DescribeSecret does name the new key, so the divergence begins after that. Hypothesis: the non-convergent wait is the final bounded loop that polls GetSecretValue for DecryptionFailure after the new key is disabled; the earlier bounded wait in the same check polls kms:Encrypt under the disabled key for DisabledException, so if KMS does not honor the disable either, that probe is the first to hang.
Why: the secret's encryption context is decorative. A client validating a CMK migration, a key-disable blast radius, or a break-glass procedure gets a success from the emulator in every case, including the case AWS refuses — so a misconfigured or revoked key is discovered only in production, and envelope-encryption failures are never exercised.
Fix: the KmsKeyId change path of the UpdateSecret handler and the decrypt step of the value-read path — services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/update_secret.rs (the `input.kms_key_id` branch) plus the GetSecretValue read path, which must consult the named CMK's state and map an unusable key to the modeled DecryptionFailure.
Traces: secretsmanager.update-secret-kms-key-change-re-encrypts-so-the-old-key-is-no-longer-needed.

=== Cause 3 of 3: UpdateSecretVersionStage removes AWSCURRENT outright instead of refusing ===
AWS: Smithy: UpdateSecretVersionStage — "You can move the AWSCURRENT staging label to this version by including it in this call" and "Whenever you move AWSCURRENT, Secrets Manager automatically moves the label AWSPREVIOUS to the version that AWSCURRENT was removed from"; a secret must always have exactly one AWSCURRENT, so AWSCURRENT can only be MOVED, never detached. A call naming VersionStage="AWSCURRENT" and RemoveFromVersionId=<the current holder> with no MoveToVersionId is refused with the modeled InvalidParameterException (HTTP 400) and the message "You can only move staging label AWSCURRENT to a different secret version. It can't be completely removed." — fixed text, with the staging label taken from the request's VersionStage member.
Emulator: the call SUCCEEDS. Replay: "expected the call to be rejected, but it succeeded". The handler's remove-only branch checks only that RemoveFromVersionId names the version the label is actually on and then detaches it, with no guard for the reserved AWSCURRENT label — leaving the secret with no current version, a state AWS cannot represent.
Why: an over-permissive accept that corrupts the secret. Every subsequent GetSecretValue with no VersionId/VersionStage has no version to resolve, so a client can drive the emulated secret into a currentless state that production refuses, and code paths guarding against it are never exercised.
Fix: the validation of the remove-only branch in services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/update_secret_version_stage.rs (the `else if let Some(remove_from)` arm, which must refuse before `detach_stage` when the label is AWSCURRENT).
Traces: secretsmanager.update-secret-version-stage-remove-current-without-move-fails.

## Evidence
Suite: secretsmanager 84/183 (46%). All three checks are in the failing list, unmarked by ALREADY FILED / OWNED by.

[mismatch] secretsmanager.update-secret-evicts-a-separately-staged-awspending-label
  AssertionError: UpdateSecret must EVICT the separately-staged AWSPENDING version v2 (<version id>) from the staging index entirely; expected no entry, got ['AWSPENDING']
  Sequence: CreateSecret(SecretString="v1") -> PutSecretValue(SecretString="pending-v2", VersionStages=["AWSPENDING"]) -> UpdateSecret(new value). Expected afterwards: the pending version carries no staging labels; its bytes remain readable by VersionId. Actual: it still carries AWSPENDING.

[mismatch] secretsmanager.update-secret-kms-key-change-re-encrypts-so-the-old-key-is-no-longer-needed
  Error: Check timed out after 60.0s
  Sequence: CreateKey(old), CreateKey(new); CreateSecret(KmsKeyId=old, SecretString=<value>); UpdateSecret(KmsKeyId=new); DescribeSecret -> names the new key (passes); DisableKey(old) + wait; GetSecretValue -> must still return <value>; GetSecretValue(VersionStage="AWSPENDING") -> must be ResourceNotFoundException "Secrets Manager can't find the specified secret value for staging label: AWSPENDING"; DisableKey(new) + wait; GetSecretValue -> must fail with code DecryptionFailure within 180 s.
  Actual: no bounded wait in the check ever converges, so it is killed at the 60 s cap. services/.../src/ops/update_secret.rs assigns `secret.kms_key_id` and makes no KMS call on write; the read path returns the stored plaintext unconditionally, so DecryptionFailure is never produced after the new key is disabled.

[mismatch] secretsmanager.update-secret-version-stage-remove-current-without-move-fails
  AssertionError: expected the call to be rejected, but it succeeded
  Call: UpdateSecretVersionStage(SecretId=<name>, VersionStage="AWSCURRENT", RemoveFromVersionId=<the version currently holding AWSCURRENT>), with no MoveToVersionId.
  Expected: InvalidParameterException (HTTP 400), message "You can only move staging label AWSCURRENT to a different secret version. It can't be completely removed.", and AWSCURRENT left unmoved (verified by a following GetSecretValue).
  Actual: HTTP 200; the label is detached. The check's own positive control (VersionStage="AWSCURRENT", MoveToVersionId=v2, RemoveFromVersionId=v1) succeeds first, proving the operation is not simply refusing everything.

## Steps to Reproduce
Cause 1:
1. CreateSecret(Name=<n>, SecretString="v1") -> note VersionId v1.
2. PutSecretValue(SecretId=<n>, SecretString="pending-v2", VersionStages=["AWSPENDING"]) -> note VersionId v2.
3. UpdateSecret(SecretId=<n>, SecretString="v3").
4. DescribeSecret(SecretId=<n>) and inspect VersionIdsToStages: AWS has no entry for v2 at all (the label was evicted); the emulator still lists v2 -> ['AWSPENDING'].
5. GetSecretValue(SecretId=<n>, VersionId=v2) still returns "pending-v2" on AWS — the bytes survive, only the label is gone.

Cause 2:
1. kms CreateKey twice (SYMMETRIC_DEFAULT / ENCRYPT_DECRYPT) -> old_key, new_key.
2. CreateSecret(KmsKeyId=old_key, SecretString="value-under-the-first-key").
3. UpdateSecret(SecretId=<arn>, KmsKeyId=new_key); DescribeSecret -> KmsKeyId == new_key (correct today).
4. kms DisableKey(old_key); poll kms Encrypt(old_key) until DisabledException; GetSecretValue -> must still return "value-under-the-first-key".
5. GetSecretValue(SecretId=<arn>, VersionStage="AWSPENDING") -> must be ResourceNotFoundException with "Secrets Manager can't find the specified secret value for staging label: AWSPENDING".
6. kms DisableKey(new_key); poll kms Encrypt(new_key) until DisabledException; poll GetSecretValue -> AWS fails with code DecryptionFailure. The emulator keeps returning 200 with the plaintext, so the poll never terminates.

Cause 3:
1. CreateSecret(Name=<n>, SecretString="v1") -> VersionId v1.
2. PutSecretValue(SecretId=<n>, SecretString="v2", VersionStages=["AWSPENDING"]) -> VersionId v2.
3. UpdateSecretVersionStage(SecretId=<n>, VersionStage="AWSCURRENT", MoveToVersionId=v2, RemoveFromVersionId=v1) -> succeeds; GetSecretValue returns v2 (positive control).
4. UpdateSecretVersionStage(SecretId=<n>, VersionStage="AWSCURRENT", RemoveFromVersionId=v2) with NO MoveToVersionId -> observe HTTP 200; AWS refuses InvalidParameterException "You can only move staging label AWSCURRENT to a different secret version. It can't be completely removed."
5. GetSecretValue(SecretId=<n>) -> on AWS still resolves v2; on the emulator the secret now has no AWSCURRENT holder.
