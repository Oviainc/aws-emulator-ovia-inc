Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__kms__secretsmanager.ecsm-aws-managed-key-secret-readable-by-role-with-no-kms-permission ec2__iam__kms__secretsmanager.ecsm-decrypt-granted-on-another-key-still-denied ec2__iam__kms__secretsmanager.ecsm-decrypt-granted-on-the-key-alias-arn-does-not-unlock-the-secret ec2__iam__kms__secretsmanager.ecsm-encryption-context-condition-scopes-decrypt-to-one-secret-denied ec2__iam__kms__secretsmanager.ecsm-get-secret-value-as-role-without-kms-decrypt-denied ec2__iam__kms__secretsmanager.ecsm-key-policy-naming-the-role-unlocks-secret-without-identity-kms-grant ec2__iam__kms__secretsmanager.ecsm-put-secret-value-with-decrypt-only-denied ec2__iam__kms__secretsmanager.ecsm-identity-deny-on-decrypt-overrides-key-policy-allow-denied ec2__iam__kms__secretsmanager.ecsm-batch-get-secret-value-reports-the-kms-denied-secret-as-a-per-secret-error ec2__iam__kms__secretsmanager.ecsm-create-secret-replayed-with-its-token-and-a-changed-value-rejected
Issue-Key: ec2__iam__kms__secretsmanager.ecsm-aws-managed-key-secret-readable-by-role-with-no-kms-permission|_refusal#13|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: ec2__iam__kms__secretsmanager.ecsm-decrypt-granted-on-another-key-still-denied|_refusal#12|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: ec2__iam__kms__secretsmanager.ecsm-decrypt-granted-on-the-key-alias-arn-does-not-unlock-the-secret|_refusal#16|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: ec2__iam__kms__secretsmanager.ecsm-encryption-context-condition-scopes-decrypt-to-one-secret-denied|_refusal#12|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: ec2__iam__kms__secretsmanager.ecsm-get-secret-value-as-role-without-kms-decrypt-denied|_refusal#11|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: ec2__iam__kms__secretsmanager.ecsm-key-policy-naming-the-role-unlocks-secret-without-identity-kms-grant|_refusal#8|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: ec2__iam__kms__secretsmanager.ecsm-put-secret-value-with-decrypt-only-denied|_refusal#10|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: ec2__iam__kms__secretsmanager.ecsm-identity-deny-on-decrypt-overrides-key-policy-allow-denied|_poll#14|Timeout|time.sleep(interval_s)
Issue-Key: ec2__iam__kms__secretsmanager.ecsm-batch-get-secret-value-reports-the-kms-denied-secret-as-a-per-secret-error|run#12|AssertionError|assert values == [(managed_secret, "batch-managed")], (
Issue-Key: ec2__iam__kms__secretsmanager.ecsm-create-secret-replayed-with-its-token-and-a-changed-value-rejected|run#11|AssertionError|assert got == want, f"the token replayed with a changed value was answered {got!r}, expected {want!r}"
Issue-Commit: 3879dd05c1f9087ace2750498c91b002c8086d49

# secretsmanager: GetSecretValue/PutSecretValue never check the caller's KMS permission on the secret's CMK, BatchGetSecretValue returns the denied secret, CreateSecret token replay message wrong

> _Note: this issue was identified against the code base at commit `3879dd05c1f9087ace2750498c91b002c8086d49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSecretValue

## Description
This issue carries 3 root causes in Secrets Manager. Not covered by 20261022T235108Z-ec2-neighbouring-service-fidelity-gaps-reached-through-ec2-led.md or 20261024T081028Z-secretsmanager-secrets-manager-getsecretvalue-reports-resourcenotfoundexception-for-a-missing.md, which carry other checks' replay keys.

Cause 1 of 3: reading or writing a secret encrypted with a customer-managed KMS key never evaluates the caller's permission on that key.
AWS: GetSecretValue/PutSecretValue on a secret encrypted with a CMK require the caller to be allowed kms:Decrypt (and kms:GenerateDataKey for a write) on that key, granted by its identity policy, the key policy or a grant, with the encryption context the secret supplies (SecretARN/SecretVersionId). A caller without it is refused — the model types the failure as DecryptionFailure/InvalidRequestException (Smithy: GetSecretValue declares DecryptionFailure, InvalidRequestException, InvalidParameterException, ResourceNotFoundException, InternalServiceError) and the recordings show an AccessDenied-class refusal naming kms:Decrypt. Scoping matters: a decrypt grant on ANOTHER key does not unlock this secret; a grant naming the key's ALIAS ARN does not unlock it; an encryption-context condition naming one secret's ARN does not unlock a second secret; an identity Deny on kms:Decrypt overrides a key-policy Allow; and conversely a key policy naming the role unlocks the secret with no identity KMS grant. A secret under the aws/secretsmanager managed key is readable without any customer KMS permission.
Emulator: every one of these GetSecretValue/PutSecretValue calls returns 200 with the plaintext SecretString (or a new version id), and the identity-deny case never converges and times out at 60 s. No KMS authorization happens at all.
Why: the KMS envelope is not a control here — a principal that real AWS refuses reads and overwrites secret material, so no least-privilege secret design can be validated against the emulator.
Fix: the GetSecretValue/PutSecretValue handlers must authorize the secret's KmsKeyId through the KMS authorizer (identity policy, key policy, grants, alias resolution and the SecretARN encryption-context condition) before returning or writing material.
Traces: ec2__iam__kms__secretsmanager.ecsm-aws-managed-key-secret-readable-by-role-with-no-kms-permission, ec2__iam__kms__secretsmanager.ecsm-decrypt-granted-on-another-key-still-denied, ec2__iam__kms__secretsmanager.ecsm-decrypt-granted-on-the-key-alias-arn-does-not-unlock-the-secret, ec2__iam__kms__secretsmanager.ecsm-encryption-context-condition-scopes-decrypt-to-one-secret-denied, ec2__iam__kms__secretsmanager.ecsm-get-secret-value-as-role-without-kms-decrypt-denied, ec2__iam__kms__secretsmanager.ecsm-key-policy-naming-the-role-unlocks-secret-without-identity-kms-grant, ec2__iam__kms__secretsmanager.ecsm-put-secret-value-with-decrypt-only-denied, ec2__iam__kms__secretsmanager.ecsm-identity-deny-on-decrypt-overrides-key-policy-allow-denied.

Cause 2 of 3: BatchGetSecretValue returns the value of a secret the caller may not decrypt instead of reporting it per secret.
AWS: BatchGetSecretValue is partial-failure: a secret whose CMK the caller cannot use is omitted from SecretValues and reported as an entry in Errors (Smithy: BatchGetSecretValueResponse has SecretValues plus Errors of APIErrorListType), while the readable secret is returned normally.
Emulator: returns both secrets' values in SecretValues and an empty Errors list.
Why: a caller that relies on Errors to detect a partially denied batch sees none, and material it is not entitled to is disclosed.
Fix: the BatchGetSecretValue handler must authorize each secret individually and route refusals into Errors.
Traces: ec2__iam__kms__secretsmanager.ecsm-batch-get-secret-value-reports-the-kms-denied-secret-as-a-per-secret-error.

Cause 3 of 3: CreateSecret replayed with the same ClientRequestToken but a changed value is refused with the wrong message.
AWS: ClientRequestToken is the operation's idempotency token (Smithy: CreateSecret.ClientRequestToken carries @idempotencyToken), and replaying it with a different SecretString is refused ResourceExistsException (Smithy-declared) with "You can't use ClientRequestToken <token> because that value is already in use for a version of secret <secret-arn>." — fixed text around the token from the request and the secret's ARN from state. The refusal is about the TOKEN, not the name.
Emulator: answers ResourceExistsException with 'The operation failed because the secret <name> already exists.' — the plain name-collision message.
Why: a client that distinguishes a name collision from an idempotency-token reuse (to decide whether to re-read the existing secret or to mint a new token) cannot tell them apart.
Fix: the CreateSecret handler's idempotency-token branch and its message.
Traces: ec2__iam__kms__secretsmanager.ecsm-create-secret-replayed-with-its-token-and-a-changed-value-rejected.

## Evidence
Cause 1: 'real AWS refuses this call, but it SUCCEEDED and returned {... "SecretString": "under-the-cmk" ...}' (and the analogous 200s for the alias, other-key, encryption-context and key-policy cases), 'real AWS refuses this call, but it SUCCEEDED and returned {... VersionId ... }' for PutSecretValue, and 'Error: Check timed out after 60.0s' for the identity-deny case.
Cause 2: "only the aws/secretsmanager secret may be returned: [('arn:...secret:sim2real-ecsm-batch-managed-...', 'batch-managed'), ('arn:...secret:sim2real-ecsm-batch-cmk-...', 'batch-cmk')]".
Cause 3: "answered ('ResourceExistsException', 'The operation failed because the secret <name> already exists.'), expected ('ResourceExistsException', \"You can't use ClientRequestToken <token> because that value is already in use for a version of secret <secret-arn>.\")".

## Steps to Reproduce
1. Cause 1: create a CMK and a secret encrypted with it; assume a role with secretsmanager:GetSecretValue but no kms:Decrypt on that key and call GetSecretValue; repeat with a decrypt grant on another key, on the key's alias ARN, with an encryption-context condition naming a different secret's ARN, with an identity Deny over a key-policy Allow, and with PutSecretValue under a decrypt-only grant.
2. Cause 2: BatchGetSecretValue over one aws/secretsmanager secret and one CMK secret the caller cannot decrypt.
3. Cause 3: CreateSecret with an explicit ClientRequestToken, then CreateSecret again with the same token and a different SecretString.
4. Or replay the check ids in trace_ids.
