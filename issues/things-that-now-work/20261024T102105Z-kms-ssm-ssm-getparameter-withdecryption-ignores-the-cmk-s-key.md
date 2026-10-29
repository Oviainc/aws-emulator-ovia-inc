Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__ssm.encryption-context-parameter-arn-condition-scopes-decryption
Issue-Key: kms__ssm.encryption-context-parameter-arn-condition-scopes-decryption|run#32|AssertionError|assert code == "AccessDeniedException", (
Issue-Commit: aa31aa657622ba455ff8faedd6354f5956e3380b

# SSM GetParameter(WithDecryption) ignores the CMK's key-policy EncryptionContext condition: a SecureString outside the permitted ARN scope is decrypted anyway (1 cause)

> _Note: this issue was identified against the code base at commit `aa31aa657622ba455ff8faedd6354f5956e3380b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetParameter

## Description
Cause 1 of 1 — the kms:EncryptionContext:PARAMETER_ARN condition on the key policy is not enforced on the SSM decrypt path.
AWS: SSM decrypts a SecureString by calling KMS with an encryption context that includes the parameter's ARN (PARAMETER_ARN). A key policy statement whose Condition restricts kms:EncryptionContext:PARAMETER_ARN to one parameter therefore admits GetParameter(WithDecryption=True) for that parameter and refuses it for any other parameter encrypted under the same CMK — the refusal surfaces to the caller as AccessDeniedException (HTTP 400), while the in-scope parameter still decrypts. Recorded behavior: the out-of-scope read is refused on every attempt; the in-scope twin succeeds.
Emulator: the out-of-scope GetParameter(WithDecryption=True) SUCCEEDS ('<call succeeded>'), so the condition has no effect — either SSM does not pass an encryption context to KMS, or KMS does not evaluate the key policy's condition (the latter is reported for service 'kms' as the missing authorization gate; this check establishes the SSM-side contract independently).
Why: parameter-level isolation on a shared CMK — the standard way to let many parameters share one key while scoping decryption per parameter — cannot be built or tested; every tenant can read every other tenant's SecureString in the emulator.
Fix: SSM's SecureString decrypt path must call KMS with the PARAMETER_ARN encryption context, and KMS's key-policy evaluation must honour kms:EncryptionContext:* conditions.

## Evidence
kms__ssm.encryption-context-parameter-arn-condition-scopes-decryption: "AssertionError: a parameter whose ARN does not satisfy the EncryptionContext condition must be refused decryption with AccessDeniedException; got '<call succeeded>': ''". Expected: HTTP 400 with Error.Code AccessDeniedException for the out-of-scope parameter, and a successful decrypt for the in-scope one.

## Steps to Reproduce
1. kms:CreateKey; PutKeyPolicy adding an Allow of kms:Decrypt for the caller with Condition StringEquals on 'kms:EncryptionContext:PARAMETER_ARN' naming parameter A's ARN only. 2. ssm:PutParameter twice (A and B) with Type='SecureString' and KeyId=<that key>. 3. ssm:GetParameter(Name=A, WithDecryption=True) — must succeed. 4. ssm:GetParameter(Name=B, WithDecryption=True) — must be AccessDeniedException; the emulator returns the decrypted value.
