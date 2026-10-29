Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.create-secret-invalid-kms-key-rejected secretsmanager.update-secret-invalid-kms-key-rejected secretsmanager.update-secret-nonexistent-kms-key-rejected secretsmanager.create-secret-name-scheduled-for-deletion-fails secretsmanager.tag-resource-invalid-key-format-rejected secretsmanager.update-secret-version-stage-nonexistent-version-id-rejected
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager.create-secret-invalid-kms-key-rejected|_refusal#6|AssertionError|raise AssertionError(f"expected rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: secretsmanager.update-secret-invalid-kms-key-rejected|_error_code#10|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: secretsmanager.update-secret-nonexistent-kms-key-rejected|_error_code#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: secretsmanager.create-secret-name-scheduled-for-deletion-fails|run#7|AssertionError|assert message == (
Issue-Key: secretsmanager.tag-resource-invalid-key-format-rejected|run#8|AssertionError|assert message == "You can't modify system tags. Tag key: 'aws:reserved-key'.", (
Issue-Key: secretsmanager.update-secret-version-stage-nonexistent-version-id-rejected|run#11|AssertionError|assert message == expected, f"rejection message {message!r}, expected {expected!r}"
Issue-Commit: d5ace36132e41d92d7f91429edc3b2c2caee7d54

# Secrets Manager: KmsKeyId is never validated against KMS on CreateSecret/UpdateSecret, and three refusal messages diverge from AWS's wording (4 causes)

> _Note: this issue was identified against the code base at commit `d5ace36132e41d92d7f91429edc3b2c2caee7d54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSecret

## Description
Four net-new root causes in the secretsmanager crate, all reached through plain single-service calls. Cause 1 is an overly-permissive accept; causes 2-4 are refusals that carry the right code but not AWS's message text. A fixer must close all four before submitting.

Cause 1 of 4: CreateSecret and UpdateSecret accept a KmsKeyId that names no KMS key.
AWS: Smithy: CreateSecret.KmsKeyId and UpdateSecret.KmsKeyId both target KmsKeyIdType (@length 0-2048); both operations model InvalidParameterException (client error, HTTP 400). Secrets Manager resolves the named key through KMS up front, so a well-formed but nonexistent key ARN is refused with InvalidParameterException and the message template "The operation failed because of an invalid KMS key: Key '<KmsKeyId exactly as supplied in the request>' does not exist (Service: AWSKMS; Status Code: 400; Error Code: NotFoundException; Request ID: <the downstream KMS call's per-request request id>; Proxy: null)". Everything outside the two angle-bracketed slots is fixed text; the key ARN comes from the request and the request id is per-run. No secret is created for a refused CreateSecret (a following DescribeSecret on that name answers ResourceNotFoundException with "Secrets Manager can't find the specified secret."), and a refused UpdateSecret leaves the secret's existing KmsKeyId unchanged.
Emulator: both calls answer HTTP 200. CreateSecret mints the secret under the nonexistent key and returns ARN/Name/VersionId; UpdateSecret returns 200 with ARN/Name and stores the bogus key id. No KMS lookup happens at all.
Why: a client that catches InvalidParameterException to report a mistyped or deleted CMK sees success instead, and is handed a secret bound to a key that does not exist - every subsequent encrypt/decrypt of that secret is then operating on a fiction. Guard code that treats "create succeeded" as "the CMK is usable" silently ships a broken configuration.
Fix: KmsKeyId validation in the CreateSecret and UpdateSecret handlers of the secretsmanager crate - resolve the key id/ARN against KMS state before the secret record is written, and refuse with InvalidParameterException carrying the KMS not-found text above.
Traces: secretsmanager.create-secret-invalid-kms-key-rejected, secretsmanager.update-secret-invalid-kms-key-rejected, secretsmanager.update-secret-nonexistent-kms-key-rejected.

Cause 2 of 4: CreateSecret's "already scheduled for deletion" refusal interpolates the secret name into text AWS keeps fixed.
AWS: recorded behavior only (the model states only the code: CreateSecret models InvalidRequestException, client error, HTTP 400). Re-using the name of a secret that is scheduled for deletion is refused with InvalidRequestException, HTTP 400, and the message is wholly fixed text: "You can't create this secret because a secret with this name is already scheduled for deletion." - no part of it is taken from the request.
Emulator: the code is right, but the message substitutes the requested name for the words "this name": "You can't create this secret because a secret with the name <Name from the request> is already scheduled for deletion." (the recorded run shows the per-run name sim2real-behav-secret-d438c50f6aab in that slot; the name is a value, the wording around it is the requirement).
Why: callers and tooling that match or display this exact sentence (and SDK error-message assertions) do not recognise the emulator's variant, so a name still held by a recovery window is reported as an unknown failure rather than the documented one.
Fix: the refusal text in the CreateSecret handler's scheduled-for-deletion branch.
Traces: secretsmanager.create-secret-name-scheduled-for-deletion-fails.

Cause 3 of 4: TagResource's reserved aws: prefix refusal uses its own wording.
AWS: Smithy: TagResource models InvalidParameterException (client error, HTTP 400). A tag key beginning with the reserved aws: prefix is refused with InvalidParameterException and the message "You can't modify system tags. Tag key: '<offending tag key from the request>'." - fixed text "You can't modify system tags. Tag key: '" + the key + "'."
Emulator: correct code InvalidParameterException, but the message is 'Invalid Tags. The prefix aws: is reserved for Amazon Web Services tags; got key "<key>".' - different fixed text and different quoting of the key. (The open issue 20261024T190718Z-secretsmanager-secrets-manager-refusals-pick-the-wrong-exception-code.md owns the sibling check tag-resource-invalid-tag-key-format-rejected, which is about the exception CODE; this message-text divergence on the aws:-prefix path is not covered by it.)
Why: a client keying off AWS's system-tag sentence cannot tell a reserved-prefix rejection from any other invalid-parameter rejection.
Fix: the reserved-prefix refusal text in the TagResource handler's tag validation.
Traces: secretsmanager.tag-resource-invalid-key-format-rejected.

Cause 4 of 4: UpdateSecretVersionStage's unknown-MoveToVersionId refusal uses the "secret version" wording instead of AWS's "secret value for VersionId".
AWS: Smithy: UpdateSecretVersionStage models ResourceNotFoundException (client error, HTTP 400). A MoveToVersionId that exists on no version of the secret is refused with ResourceNotFoundException and the message "Secrets Manager can't find the specified secret value for VersionId: <MoveToVersionId from the request>" - fixed prefix "Secrets Manager can't find the specified secret value for VersionId: " followed by the version id supplied in the request. The staging labels are left untouched (AWSCURRENT stays on the original version).
Emulator: right code, message "Secrets Manager can't find the specified secret version: <MoveToVersionId>" - the fixed text is wrong ("secret version:" instead of "secret value for VersionId:").
Why: rotation code and operators distinguish "no such secret" from "no such version value" by this exact sentence; the emulator's wording breaks that distinction.
Fix: the not-found refusal text in the UpdateSecretVersionStage handler's MoveToVersionId resolution.
Traces: secretsmanager.update-secret-version-stage-nonexistent-version-id-rejected.

## Evidence
Cause 1 - secretsmanager.create-secret-invalid-kms-key-rejected: expected a refusal, got HTTPStatusCode 200 and {'ARN': 'arn:aws:secretsmanager:us-west-2:000000000000:secret:sim2real-behav-badkms-...', 'Name': 'sim2real-behav-badkms-...', 'VersionId': '2a4ea1b9-...'} for CreateSecret(Name=..., SecretString='x', KmsKeyId='arn:aws:kms:us-west-2:<account>:key/00000000-0000-0000-0000-000000000000'). Expected: 400 InvalidParameterException, message "The operation failed because of an invalid KMS key: Key '<that ARN>' does not exist (Service: AWSKMS; Status Code: 400; Error Code: NotFoundException; Request ID: <id>; Proxy: null)".
secretsmanager.update-secret-invalid-kms-key-rejected / .update-secret-nonexistent-kms-key-rejected: UpdateSecret(SecretId=<arn>, KmsKeyId=<nonexistent key arn>) answered HTTPStatusCode 200 with {'ARN': ..., 'Name': ...}; expected the same 400 InvalidParameterException refusal and KmsKeyId unchanged.

Cause 2 - secretsmanager.create-secret-name-scheduled-for-deletion-fails: after DeleteSecret(RecoveryWindowInDays=7), CreateSecret with the same name was refused but with message "You can't create this secret because a secret with the name sim2real-behav-secret-d438c50f6aab is already scheduled for deletion."; expected exactly "You can't create this secret because a secret with this name is already scheduled for deletion."

Cause 3 - secretsmanager.tag-resource-invalid-key-format-rejected: TagResource(Tags=[{Key:'aws:reserved-key',Value:'nope'}]) returned InvalidParameterException with message 'Invalid Tags. The prefix aws: is reserved for Amazon Web Services tags; got key "aws:reserved-key".'; expected "You can't modify system tags. Tag key: 'aws:reserved-key'."

Cause 4 - secretsmanager.update-secret-version-stage-nonexistent-version-id-rejected: UpdateSecretVersionStage(VersionStage='AWSCURRENT', MoveToVersionId=<random uuid>, RemoveFromVersionId=<v0>) returned ResourceNotFoundException with message "Secrets Manager can't find the specified secret version: 0bc648a7-c1f5-41ee-9389-0320817c495b"; expected "Secrets Manager can't find the specified secret value for VersionId: 0bc648a7-c1f5-41ee-9389-0320817c495b".

## Steps to Reproduce
Replay the behavioral suite and re-run exactly these checks:
1. secretsmanager.create-secret-invalid-kms-key-rejected - CreateSecret(Name=<new>, SecretString='x', KmsKeyId='arn:aws:kms:<region>:<account>:key/00000000-0000-0000-0000-000000000000'); expect 400 InvalidParameterException with the KMS not-found text, then DescribeSecret(<that name>) -> ResourceNotFoundException.
2. secretsmanager.update-secret-nonexistent-kms-key-rejected / secretsmanager.update-secret-invalid-kms-key-rejected - CreateSecret a normal secret, then UpdateSecret(SecretId=<arn>, KmsKeyId=<well-formed nonexistent key arn>); expect the same 400 refusal and DescribeSecret(<arn>).KmsKeyId unchanged; a plain UpdateSecret(Description=...) on the same secret must still succeed.
3. secretsmanager.create-secret-name-scheduled-for-deletion-fails - CreateSecret(Name=N), DeleteSecret(SecretId=N, RecoveryWindowInDays=7), CreateSecret(Name=N) again; expect InvalidRequestException with the fixed sentence containing "a secret with this name".
4. secretsmanager.tag-resource-invalid-key-format-rejected - CreateSecret, then TagResource(SecretId=<arn>, Tags=[{Key:'aws:reserved-key',Value:'nope'}]); expect InvalidParameterException "You can't modify system tags. Tag key: 'aws:reserved-key'."
5. secretsmanager.update-secret-version-stage-nonexistent-version-id-rejected - CreateSecret, then UpdateSecretVersionStage(SecretId=<arn>, VersionStage='AWSCURRENT', MoveToVersionId=<unused uuid>, RemoveFromVersionId=<created VersionId>); expect ResourceNotFoundException with "...specified secret value for VersionId: <uuid>".
