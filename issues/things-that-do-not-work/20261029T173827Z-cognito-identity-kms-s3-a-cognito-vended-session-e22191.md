Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity__kms__s3.xsvc-cognito-identity-1-guest-session-denied-sse-kms-read-key-policy-grants-authenticated-only cognito-identity__kms__s3.xsvc-cognito-identity-1-sse-kms-get-object-denied-to-vended-session-until-key-policy-grants-role
Issue-Key: cognito-identity__kms__s3.xsvc-cognito-identity-1-guest-session-denied-sse-kms-read-key-policy-grants-authenticated-only|run#43|AssertionError|assert err is not None, (
Issue-Key: cognito-identity__kms__s3.xsvc-cognito-identity-1-sse-kms-get-object-denied-to-vended-session-until-key-policy-grants-role|run#39|AssertionError|assert err is not None, (
Issue-Commit: c2c15166cf9b7cee19d5a54b2d3b1c8d8385cb40

# cognito-identity__kms__s3: a Cognito-vended session reads an SSE-KMS object although the KMS key policy grants kms:Decrypt to nobody it is part of

> _Note: this issue was identified against the code base at commit `c2c15166cf9b7cee19d5a54b2d3b1c8d8385cb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Cause 1 of 1 — SSE-KMS GetObject by a Cognito-vended session is not gated on kms:Decrypt.
AWS: recorded behavior only: reading an object encrypted with a customer-managed KMS key requires the caller's principal to reach kms:Decrypt on that key. A guest (unauthenticated-role) session is refused with AccessDenied when the key policy grants kms:Decrypt to the authenticated role only, and a vended session with no path to kms:Decrypt is refused until the key policy grants its role — S3 surfaces KMS's denial rather than returning the body.
Emulator: GetObject returns the decrypted object body in both cases — the key policy is never evaluated for the vended session.
Why: encryption-at-rest with a key policy is the standard way to separate tenants and privilege levels behind one bucket; here the key policy grants nothing and withholds nothing, so any session that can reach the object can read plaintext. Any test that proves data is protected by a key policy passes vacuously. Related open issue 20261025T005354Z-cognito-identity-kms-s3-a-cognito-vended-session.md covers only the kms:EncryptionContext condition case and does not carry these two checks' keys.
Fix: KMS authorization evaluation on the S3 SSE-KMS decrypt path — the GetObject handler must call the key's policy evaluation with the requesting session's principal (the mapped role session vended by Cognito) and surface AccessDenied when it is denied.
Traces: cognito-identity__kms__s3.xsvc-cognito-identity-1-guest-session-denied-sse-kms-read-key-policy-grants-authenticated-only, cognito-identity__kms__s3.xsvc-cognito-identity-1-sse-kms-get-object-denied-to-vended-session-until-key-policy-grants-role.

## Evidence
"the key policy grants kms:Decrypt to the authenticated role only, yet a guest session read the SSE-KMS object"; "a vended session with no path to kms:Decrypt must not read an SSE-KMS object, but GetObject returned its body".

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-replay cognito-identity__kms__s3.xsvc-cognito-identity-1-guest-session-denied-sse-kms-read-key-policy-grants-authenticated-only and cognito-identity__kms__s3.xsvc-cognito-identity-1-sse-kms-get-object-denied-to-vended-session-until-key-policy-grants-role.
3. By hand: create a CMK whose key policy grants kms:Decrypt to the identity pool's AUTHENTICATED role only; PutObject with SSE-KMS under that key; vend guest credentials via GetId/GetCredentialsForIdentity on the same pool and GetObject the key — expect AccessDenied, not the body.
