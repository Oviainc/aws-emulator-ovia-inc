Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__secretsmanager.secret-under-a-disabled-cmk-is-unreadable-until-the-key-is-re-enabled
Issue-Key: kms__secretsmanager.secret-under-a-disabled-cmk-is-unreadable-until-the-key-is-re-enabled|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# kms__secretsmanager: GetSecretValue and PutSecretValue never consult the secret's CMK key state — a secret under a DISABLED CMK stays readable and writable instead of failing DecryptionFailure/EncryptionFailure (1 cause)

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSecretValue

## Description
ONE cause. The check polls for the refusal AWS produces and never sees it, so it is killed at the suite's 60 s cap; a faithful target refuses on the first probe (the check allows 90 s only as propagation margin), so the timeout is the observation.

Related OPEN issues that do NOT carry this check's replay key (named for context only): 20261024T102105Z-kms-secretsmanager-secrets-manager-getsecretvalue-keeps-succeeding-after-the-caller.md (the caller's kms:Decrypt PERMISSION removed — the cause below is the key's STATE), 20261024T190718Z-secretsmanager-secrets-manager-enforces-no-authorization-and-never-consults.md and 20261025T173233Z-secretsmanager-secrets-manager-updatesecret-updatesecretversionstage-awspending-is-not-evicted.md.

Cause 1 of 1 — a secret encrypted with a customer CMK keeps decrypting and re-encrypting after DisableKey.
AWS (recorded behavior for the codes; the Secrets Manager model carries no key-state semantics): once the secret's CMK reports KeyState Disabled, GetSecretValue fails with Secrets Manager's own DecryptionFailure — not a generic InvalidRequestException or AccessDeniedException — and relays the wrapped KMS DisabledException in its message; PutSecretValue on the same secret fails with the mirror code EncryptionFailure and stores no new version. After EnableKey the identical GetSecretValue succeeds again and returns the original SecretString.
Emulator: GetSecretValue keeps returning the SecretString for the full window (the check probes every 3 s and the harness kills it at 60 s), so the KMS key state is never consulted on either the read or the write path. The leg before it passed — DescribeKey reports KeyState 'Disabled' — so the key really is disabled; Secrets Manager simply does not depend on it, which means the stored secret is not actually enveloped under that CMK.
Why: disabling the CMK is the standard break-glass control for a leaked secret. An emulator that keeps serving the plaintext makes that control look effective while it does nothing, hides the DecryptionFailure/EncryptionFailure codes clients must handle, and lets a new version be written under a key that cannot encrypt.
Fix: the Secrets Manager GetSecretValue/PutSecretValue handlers' envelope path — call KMS Decrypt/GenerateDataKey against the secret's KmsKeyId on every read/write and map the KMS refusal to DecryptionFailure / EncryptionFailure.
Traces: kms__secretsmanager.secret-under-a-disabled-cmk-is-unreadable-until-the-key-is-re-enabled (60 s timeout).

## Evidence
"Error: Check timed out after 60.0s". The surviving probe is _poll_until(lambda: _error_code(lambda: sm.get_secret_value(SecretId=arn)), lambda pair: pair[0] is not None, timeout_s=90.0, interval_s=3.0, description='GetSecretValue to start refusing once the CMK is disabled') — it returns only when the call starts failing, so every 3 s probe returned HTTP 200 with the SecretString. The immediately preceding poll (DescribeKey KeyState == 'Disabled', timeout 60 s) passed. The check's measured expectations, recorded against real AWS: read refusal code 'DecryptionFailure' relaying the wrapped KMS DisabledException in the message, write refusal code 'EncryptionFailure', and recovery (GetSecretValue succeeding again) after EnableKey.

## Steps to Reproduce
CreateKey; CreateSecret(KmsKeyId=<key ARN>, SecretString=<value>); GetSecretValue (succeeds); DisableKey and poll DescribeKey to KeyState=Disabled; GetSecretValue repeatedly — observe HTTP 200 with the SecretString instead of DecryptionFailure; PutSecretValue — observe success instead of EncryptionFailure; EnableKey and read again.
