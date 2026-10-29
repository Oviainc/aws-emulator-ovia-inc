Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__kms.sep-kms-key-scope-key-policy-deny-beats-identity-allow-on-that-key-only-denied
Issue-Key: iam__kms.sep-kms-key-scope-key-policy-deny-beats-identity-allow-on-that-key-only-denied|_rejected_error#28|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Commit: aa31aa657622ba455ff8faedd6354f5956e3380b

# A key policy's explicit Deny does not beat an identity-policy Allow: Decrypt succeeds on a key whose policy denies the caller (1 cause)

> _Note: this issue was identified against the code base at commit `aa31aa657622ba455ff8faedd6354f5956e3380b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Decrypt

## Description
Cause 1 of 1 — the key policy is not consulted when an identity policy allows the action.
AWS: KMS authorization is the union of identity policy, key policy and grants, and an explicit Deny in the key policy is final — a role whose identity policy allows kms:Decrypt on all keys is still refused AccessDeniedException (HTTP 400) on the one key whose key policy denies that principal, while the same role's Decrypt on a sibling key with no such Deny succeeds. Recorded behavior: the refusal carries code AccessDeniedException; the pass/fail pair proves the deny is scoped to one key.
Emulator: the Decrypt that must be denied returns HTTP 200 with the plaintext ('Plaintext': b'sep-deny-scope-payload-A'), i.e. the key policy's Deny statement is ignored entirely. This is the key-policy half of the same missing authorization gate reported for service 'kms' (see the companion issue 'KMS: no authorization is evaluated ...'), observed here through an IAM-role-scoped scenario; it is listed separately because its check belongs to the iam__kms pair.
Why: the single most important KMS guardrail — 'the key owner can always deny' — is unenforceable, so any policy-separation design (per-tenant key isolation, break-glass denies) tests green in the emulator and fails only in production.
Fix: the KMS authorization gate must evaluate the resource (key) policy, and an explicit Deny there must short-circuit regardless of the identity policy.

## Evidence
iam__kms.sep-kms-key-scope-key-policy-deny-beats-identity-allow-on-that-key-only-denied: "AssertionError: real AWS refuses this call, but it SUCCEEDED and returned {'KeyId': 'arn:aws:kms:us-west-2:000000000000:key/89fdbdf4-06ad-4aa7-94e7-dea6127f37a6', 'Plaintext': b'sep-deny-scope-payload-A', 'EncryptionAlgorithm': 'SYMMETRIC_DEFAULT', ... 'HTTPStatusCode': 200}". Expected: HTTP 400, Error.Code AccessDeniedException, no Plaintext in the response.

## Steps to Reproduce
1. kms:CreateKey twice (A and B). 2. iam:CreateRole with an identity policy allowing kms:Decrypt on '*'. 3. kms:PutKeyPolicy on A adding an explicit Deny of kms:Decrypt for that role's ARN. 4. Encrypt a payload under A and under B. 5. Assume the role and Decrypt both blobs — A must be AccessDeniedException while B succeeds; the emulator returns 200 for both.
