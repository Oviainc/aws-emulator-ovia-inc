Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__lambda.deep-lambda-2-env-kms-absent-key-rejected-and-state-unchanged kms__lambda.deep-lambda-2-env-kms-disable-blocks-cold-invoke-then-enable-restores kms__lambda.kms-grant-authorizes-lambda-decrypt-then-revoke-denies kms__lambda.kms-key-policy-deny-overrides-lambda-identity-allow
Issue-Key: kms__lambda.deep-lambda-2-env-kms-absent-key-rejected-and-state-unchanged|run#31|AssertionError|raise AssertionError(
Issue-Key: kms__lambda.deep-lambda-2-env-kms-disable-blocks-cold-invoke-then-enable-restores|_poll#15|Timeout|last = probe()
Issue-Key: kms__lambda.kms-grant-authorizes-lambda-decrypt-then-revoke-denies|_invoke_payload#9|AssertionError|assert "error" not in result, f"handler faulted instead of returning a result: {result!r}"
Issue-Key: kms__lambda.kms-key-policy-deny-overrides-lambda-identity-allow|_invoke_payload#8|AssertionError|assert "error" not in result, f"handler faulted instead of returning a result: {result!r}"
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# kms__lambda: environment-variable KMS enforcement and KMS grant/key-policy denials are never applied to an invocation

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1 — a function whose environment variables are encrypted with a customer managed key does not have that key's availability or authorization enforced at invoke time, and a KMS grant or key-policy deny does not change what the function may decrypt.
AWS: when KMSKeyArn is set on a function's Environment, Lambda decrypts the variables with that key on each cold start: if the key is absent, CreateFunction/UpdateFunctionConfiguration is refused with KMSNotFoundException and the function's state is unchanged; if the key is disabled, the cold invoke is refused with KMSDisabledException (502) and re-enabling it restores invocation. A kms:Decrypt grant to the execution role authorizes the decrypt and RevokeGrant withdraws it; an explicit Deny in the key policy overrides an identity-policy Allow.
Emulator: recorded behavior only — the absent-key check fails in its body rather than observing the refusal, the disable/enable check never settles ("Check timed out after 60.0s"), and the grant and key-policy-deny checks fail where they assert the decrypt outcome changed.
Why: environment-variable encryption gives no protection: a function keeps running after its key is disabled or its grant revoked, and an explicit key-policy Deny does not stop a decrypt — exactly the controls an account uses to revoke access to secrets in bulk.
Fix: the KMS decrypt of Environment.Variables in the function cold-start path, and its authorization against grants and the key policy; plus the KMSNotFoundException validation in CreateFunction/UpdateFunctionConfiguration.
Traces: all four check ids in trace_ids.

## Evidence
kms__lambda.deep-lambda-2-env-kms-absent-key-rejected-and-state-unchanged fails in its body instead of observing KMSNotFoundException; kms__lambda.deep-lambda-2-env-kms-disable-blocks-cold-invoke-then-enable-restores reports "Check timed out after 60.0s"; kms__lambda.kms-grant-authorizes-lambda-decrypt-then-revoke-denies and .kms-key-policy-deny-overrides-lambda-identity-allow fail at their assertion on the decrypt outcome. Expected: KMSNotFoundException for the absent key, KMSDisabledException (502) on a cold invoke with the key disabled, success only while the grant is live, and a key-policy Deny overriding the identity Allow.

## Steps to Reproduce
1. CreateFunction with Environment.KMSKeyArn naming a nonexistent key.
2. CreateKey, set it as KMSKeyArn, DisableKey, force a cold start and Invoke; EnableKey and Invoke again.
3. CreateGrant kms:Decrypt to the execution role, Invoke, RevokeGrant, Invoke again.
4. PutKeyPolicy with an explicit Deny for the execution role while its identity policy allows kms:Decrypt, then Invoke.
