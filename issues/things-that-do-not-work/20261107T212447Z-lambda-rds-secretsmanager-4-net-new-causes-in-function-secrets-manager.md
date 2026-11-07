Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__rds__secretsmanager.lambda-secretsmanager-iam-missing-permission-rejected lambda__rds__secretsmanager.lambda-secretsmanager-kms-decrypt-missing-permission-denied lambda__rds__secretsmanager.lambda-secretsmanager-missing-permission-rejected lambda__rds__secretsmanager.lambda-secretsmanager-resource-policy-denies-principal-rejected lambda__rds__secretsmanager.lambda-secretsmanager-wrong-secret-arn-denied lambda__rds__secretsmanager.cli-lambda-invoke-with-rds-secret-missing-permission-denied lambda__rds__secretsmanager.lambda-secretsmanager-cross-account-secret-denied lambda__rds__secretsmanager.lambda-secretsmanager-put-secret-value-positive lambda__rds__secretsmanager.rds-secretsmanager-rotation-missing-lambda-permission-negative
Issue-Key: lambda__rds__secretsmanager.lambda-secretsmanager-iam-missing-permission-rejected|run#10|AssertionError|assert denied["ok"] is False, f"expected the fetch to be denied with zero secretsmanager permissions, got {denied!r}"
Issue-Key: lambda__rds__secretsmanager.lambda-secretsmanager-kms-decrypt-missing-permission-denied|run#22|AssertionError|assert cmk_body.get("ok") is False, cmk_body
Issue-Key: lambda__rds__secretsmanager.lambda-secretsmanager-missing-permission-rejected|run#11|AssertionError|assert result["ok"] is False, f"expected the secret fetch to be denied, got {result!r}"
Issue-Key: lambda__rds__secretsmanager.lambda-secretsmanager-resource-policy-denies-principal-rejected|_poll_until#15|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s, last={last!r}")
Issue-Key: lambda__rds__secretsmanager.lambda-secretsmanager-wrong-secret-arn-denied|run#16|AssertionError|assert other_body.get("ok") is False, other_body
Issue-Key: lambda__rds__secretsmanager.cli-lambda-invoke-with-rds-secret-missing-permission-denied|run#37|AssertionError|assert deny_resp.get("FunctionError") == "Unhandled", (
Issue-Key: lambda__rds__secretsmanager.lambda-secretsmanager-cross-account-secret-denied|run#22|AssertionError|assert denied_body["error_message"] == (
Issue-Key: lambda__rds__secretsmanager.lambda-secretsmanager-put-secret-value-positive|run#12|AssertionError|assert body.get("ok") is True, body
Issue-Key: lambda__rds__secretsmanager.rds-secretsmanager-rotation-missing-lambda-permission-negative|_poll_until#31|Timeout|time.sleep(interval_s)
Issue-Commit: a5c62b28646ac18350a618623d28fc3117c37127

# 4 net-new causes: in-function Secrets Manager authorization not enforced, denial names the wrong principal, PutSecretValue refused, rotation check stalls

> _Note: this issue was identified against the code base at commit `a5c62b28646ac18350a618623d28fc3117c37127`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSecretValue

## Description
Four net-new root causes. Open issues 20261105T081139Z-lambda-rds-secretsmanager-2-net-new-root-causes-e2a51e.md and 20261104T044754Z-lambda-rds-secretsmanager-7-net-new-root-causes.md cover other checks in this service; none of the checks below carry their replay keys.

Cause 1 of 4 — a Secrets Manager call made from inside a Lambda is authorized for everyone: no identity policy, no resource policy and no KMS grant is enforced.
AWS: recorded behavior only. GetSecretValue from a function is evaluated against the execution role's identity policy, the secret's resource policy and (for a CMK-encrypted secret) kms:Decrypt on the key; any of those denying refuses the call with AccessDeniedException, and a secret ARN outside the role's Resource scope is likewise refused.
Emulator: every one of those legs succeeds and returns the plaintext. Replay details: `AssertionError: expected the fetch to be denied with zero secretsmanager permissions, got {'ok': True, 'secret_string': 'payload-...'}`; `AssertionError: {'ok': True, 'secret_string': 'cmk-protected-...'}` with no kms:Decrypt grant; `AssertionError: expected the secret fetch to be denied, got {'ok': True, 'value': 'sim2real-topsecret-value'}`; `AssertionError: condition not met within 30.0s, last={'ok': True, 'secret_string': 'payload-...'}` under a resource policy that denies the principal; `AssertionError: {'ok': True, 'secret_string': 'other-...'}` for a secret ARN the role was never granted; and `AssertionError: expected the unpermissioned invoke to raise, got FunctionError=None`.
Why: secret material is readable by any function regardless of policy. A client's least-privilege boundary is not enforced at all — a compromised or mis-scoped function reads every secret in the account, and every negative authorization test in the client's own suite passes vacuously, so the hole is invisible.
Fix: the Secrets Manager read path's authorization — evaluate the caller's identity policy, the secret's resource policy and kms:Decrypt on the encryption key before returning SecretString/SecretBinary, and refuse with AccessDeniedException otherwise.
Traces: lambda__rds__secretsmanager.lambda-secretsmanager-iam-missing-permission-rejected, lambda__rds__secretsmanager.lambda-secretsmanager-kms-decrypt-missing-permission-denied, lambda__rds__secretsmanager.lambda-secretsmanager-missing-permission-rejected, lambda__rds__secretsmanager.lambda-secretsmanager-resource-policy-denies-principal-rejected, lambda__rds__secretsmanager.lambda-secretsmanager-wrong-secret-arn-denied, lambda__rds__secretsmanager.cli-lambda-invoke-with-rds-secret-missing-permission-denied.

Cause 2 of 4 — the one denial that is produced names the wrong principal.
AWS: recorded behavior only. When a secret's resource policy explicitly denies the caller, the AccessDeniedException message names the *caller* — for a call made inside a function, the function's assumed-role session, e.g. `User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: secretsmanager:GetSecretValue on resource: <secretArn> with an explicit deny in a resource-based policy`.
Emulator: the message names a static IAM user. Replay detail: `AssertionError: {'ok': False, 'error_code': 'AccessDeniedException', 'error_message': 'User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: secretsmanager:GetSecretValue on resource: arn:aws:secretsmanager:...:secret:...-oz8xCG with an explicit deny in a resource-based policy'}` — the principal segment is `user/emulator` rather than the function's assumed-role session; the fixed text and the action/resource segments are otherwise AWS's.
Why: a client reading the denial to find out *which* principal was refused is told it was an emulator user, not its own function role, so cross-account and cross-role diagnosis is impossible and audit trails attribute the attempt to the wrong identity.
Fix: the AccessDeniedException message builder on the Secrets Manager path — substitute the resolved caller principal (the function's assumed-role session ARN) into the message.
Traces: lambda__rds__secretsmanager.lambda-secretsmanager-cross-account-secret-denied.

Cause 3 of 4 — PutSecretValue from a function is refused with InvalidRequestException.
AWS: recorded behavior only. PutSecretValue on an existing, enabled secret with a SecretString creates a new version labelled AWSCURRENT and returns 200 with ARN/Name/VersionId; InvalidRequestException is reserved for a secret that is scheduled for deletion or otherwise not in a state that permits the write.
Emulator: the write is refused. Replay detail: `AssertionError: {'ok': False, 'error_code': 'InvalidRequestException'}`.
Why: a client cannot rotate or update a secret at all — the write it is entitled to make is refused with an error code that says the secret is in a bad state, which sends the client down a deletion-recovery path that does not apply.
Fix: the PutSecretValue handler's precondition checks and its version-staging logic.
Traces: lambda__rds__secretsmanager.lambda-secretsmanager-put-secret-value-positive.

Cause 4 of 4 — the rotation-without-invoke-permission check never reaches a verdict.
AWS: recorded behavior only. RotateSecret with a RotationLambdaARN the Secrets Manager service principal may not invoke is refused, so the recording observes a refusal rather than a hang.
Emulator: `Error: Check timed out after 180.0s` — neither a refusal nor a rotation is observed within the harness deadline.
Why: a client that mis-wires a rotation Lambda's permission gets no answer at all: the rotation neither runs nor fails, so the secret silently stops rotating.
Fix: the RotateSecret handler — validate that Secrets Manager may invoke the configured rotation function and refuse promptly when it may not.
Traces: lambda__rds__secretsmanager.rds-secretsmanager-rotation-missing-lambda-permission-negative.

## Evidence
Cause 1: expected AccessDeniedException for each of (no secretsmanager grant / no kms:Decrypt on the CMK / out-of-scope secret ARN / resource policy deny); actual every leg returned {'ok': True} with the plaintext secret, and the unpermissioned invoke returned FunctionError=None.
Cause 2: expected the denial to name the function's assumed-role session; actual "User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: secretsmanager:GetSecretValue on resource: <secretArn> with an explicit deny in a resource-based policy".
Cause 3: expected PutSecretValue 200 with a new AWSCURRENT version; actual error_code InvalidRequestException.
Cause 4: expected RotateSecret to be refused without a Secrets Manager invoke permission on the rotation function; actual `Error: Check timed out after 180.0s`.

## Steps to Reproduce
Cause 1: CreateSecret (and a CMK-encrypted secret); CreateFunction whose handler calls GetSecretValue; give the execution role no secretsmanager permission (then: no kms:Decrypt; then: a Resource scoped to another secret; then: a resource policy on the secret explicitly denying the role). Invoke each — all return the plaintext.
Cause 2: as above with an explicit-deny resource policy; read the AccessDeniedException message's principal.
Cause 3: CreateSecret, then PutSecretValue(SecretId=..., SecretString=...) from the function — InvalidRequestException.
Cause 4: RotateSecret with a RotationLambdaARN that grants secretsmanager.amazonaws.com no lambda:InvokeFunction; the call neither rotates nor is refused.
