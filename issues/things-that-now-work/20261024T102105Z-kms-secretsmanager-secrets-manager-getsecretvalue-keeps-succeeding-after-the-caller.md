Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__secretsmanager.get-secret-value-denied-when-the-cmk-decrypt-permission-is-removed
Issue-Key: kms__secretsmanager.get-secret-value-denied-when-the-cmk-decrypt-permission-is-removed|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Commit: aa31aa657622ba455ff8faedd6354f5956e3380b

# Secrets Manager GetSecretValue keeps succeeding after the caller's kms:Decrypt on the secret's CMK is removed: the denial never converges and the check hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `aa31aa657622ba455ff8faedd6354f5956e3380b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSecretValue

## Description
Cause 1 of 1 — Secrets Manager does not evaluate the secret's CMK permission on the read path, so a read that must become AccessDeniedException never does.
AWS: GetSecretValue decrypts the secret's version with the secret's KMS key on the caller's behalf; once the caller loses kms:Decrypt on that CMK (key policy statement removed, or the grant revoked), the read is refused AccessDeniedException (HTTP 400) and the SecretString is not returned. The refusal becomes effective within seconds, which is why the check polls for it rather than asserting immediately.
Emulator: the read keeps returning the SecretString, so the polled-for refusal never appears and the check ends as 'Check timed out after 60.0s' with no verdict.
Why: the most common Secrets Manager break-glass control — revoke KMS access to cut off secret reads without touching the secret — has no effect, so a client cannot test or demonstrate it and the emulator reports a passing read where production denies.
Fix: the GetSecretValue handler must perform (and be gated by) the KMS decrypt authorization for the secret's KmsKeyId. Related, already open: '20261024T081028Z-secretsmanager-secrets-manager-getsecretvalue-reports-resourcenotfoundexception-for-a-missing.md' (authorization-vs-existence ordering) and '20261022T235108Z-ec2-neighbouring-service-fidelity-gaps-reached-through-ec2-led.md' (Secrets Manager ignores KMS key state and grants); neither carries this check's replay key.

## Evidence
kms__secretsmanager.get-secret-value-denied-when-the-cmk-decrypt-permission-is-removed: 'Error: Check timed out after 60.0s'. The check removes the caller's kms:Decrypt on the secret's CMK and then polls GetSecretValue for Error.Code AccessDeniedException (HTTP 400); the call continues to answer HTTP 200 with the SecretString for the whole budget.

## Steps to Reproduce
1. kms:CreateKey; secretsmanager:CreateSecret with KmsKeyId=<that key> and a SecretString. 2. GetSecretValue — succeeds (positive control). 3. Remove the caller's kms:Decrypt on the CMK (PutKeyPolicy without the statement, or RevokeGrant). 4. Poll GetSecretValue — it must become AccessDeniedException; in the emulator it keeps returning the SecretString until the check's 60 s cap.
