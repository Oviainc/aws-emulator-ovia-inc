Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity__kms__s3.xsvc-cognito-identity-1-encryption-context-scoped-key-grant-denied-vended-session-outside-its-prefix
Issue-Key: cognito-identity__kms__s3.xsvc-cognito-identity-1-encryption-context-scoped-key-grant-denied-vended-session-outside-its-prefix|_refusal#37|AssertionError|raise AssertionError(f"{why}: the call SUCCEEDED and returned {result!r}")
Issue-Commit: d0dc720ebbf11903c1fa9407d1c82e53eafd4b75

# cognito-identity__kms__s3: a Cognito-vended session reads an SSE-KMS object outside the key policy's encryption-context scope — the kms:EncryptionContext condition is never enforced (1 cause)

> _Note: this issue was identified against the code base at commit `d0dc720ebbf11903c1fa9407d1c82e53eafd4b75`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
ONE root cause (cause 1 of 1).

AWS: a KMS key policy (or grant) that admits the federated role only under a condition on the encryption context — e.g. kms:EncryptionContext:prefix = "open/" — lets the vended session decrypt objects written under open/ and refuses the Decrypt for an object written under closed/. s3:GetObject on that object therefore fails with a KMS-sourced access denial (HTTP 403 AccessDenied / KMS.AccessDeniedException in the recorded response) and returns no body. The check's in-file control (reading an open/ object) is what proves the wiring is otherwise healthy.
Emulator: s3:GetObject for the closed/ key returns HTTP 200 and the plaintext body — the encryption-context condition in the key policy is never evaluated, so the scope the key policy expresses grants nothing. This is consistent with the Cognito-vended credentials resolving to the emulator's ambient principal (arn:aws:iam::000000000000:user/emulator) instead of an assumed-role session, leaving no principal or request context for the condition to bind to.
Why: encryption-context scoping is the mechanism that keeps one tenant's vended session from decrypting another tenant's objects under a shared KMS key; with it inert, envelope-encryption isolation validated against the emulator does not hold in production.
Fix: two components: the cognito-identity GetCredentialsForIdentity handler must mint a real assumed-role session (so a principal exists), and the KMS Decrypt authorization path invoked by the S3 SSE-KMS read must evaluate key-policy/grant conditions on the encryption context (kms:EncryptionContext*/kms:EncryptionContextKeys) and refuse the read when they are not satisfied.
Related but NOT covering: open issue 20261024T102105Z-cognito-identity-kms-s3-s3-getobject-does-not-report-serversideencryption-for-an.md records a different defect in this same service group (GetObject omitting ServerSideEncryption) and does not carry this check's replay key for the authorization divergence.
Traces: cognito-identity__kms__s3.xsvc-cognito-identity-1-encryption-context-scoped-key-grant-denied-vended-session-outside-its-prefix.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1:

cognito-identity__kms__s3.xsvc-cognito-identity-1-encryption-context-scoped-key-grant-denied-vended-session-outside-its-prefix
  File "check_…_encryption_context_scoped_key_grant_denied_vended_session_outside_its_prefix.py", line 260, in run
    code, msg = _refusal(
        lambda: vs3.get_object(Bucket=bucket, Key=_CLOSED_KEY)["Body"].read(),
        "the key policy's encryption-context condition names open/* only, yet the vended session read an object under closed/",
    )
  File "…", line 86, in _refusal
    raise AssertionError(f"{why}: the call SUCCEEDED and returned {result!r}")

Expected: the GetObject is refused because the KMS Decrypt is not permitted for an encryption context outside open/*. Observed: 200 with the object body.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; re-replay the check id in trace_ids.
2. Manual repro: create a KMS key whose policy admits the Cognito federated role for kms:Decrypt only with Condition StringLike kms:EncryptionContext:<key> matching "open/*"; put two SSE-KMS objects into a bucket, one under open/ and one under closed/, each with the matching encryption context; obtain a vended session via GetId + GetCredentialsForIdentity and read both -> expect open/ to succeed and closed/ to be refused; observe both succeeding.
