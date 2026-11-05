Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__lambda__s3.sse-kms-object-unreadable-when-key-policy-denies-role
Issue-Key: kms__lambda__s3.sse-kms-object-unreadable-when-key-policy-denies-role|_invoke_payload#13|AssertionError|assert "error" not in result, f"handler faulted instead of returning a result: {result!r}"
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# kms__lambda__s3: an SSE-KMS object stays readable when the key policy denies the reading role

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Cause 1 of 1 — a key-policy Deny on the CMK that encrypts an object does not make GetObject fail for the denied role.
AWS: reading an object written with SSE-KMS requires kms:Decrypt on the CMK; when the key policy explicitly denies the reading principal, S3's GetObject is refused with AccessDenied (403) and the message names kms:Decrypt on the key, because S3 cannot obtain the data key on the caller's behalf.
Emulator: recorded behavior only — the check fails at the point where it asserts the read is refused, so the object is still served despite the key-policy Deny.
Why: KMS key policies are the mechanism an account uses to revoke access to encrypted data without touching the bucket; if they do not gate S3 reads, data believed to be locked remains readable.
Fix: the SSE-KMS decrypt authorization on the S3 GetObject path — it must consult the CMK's key policy (and grants) for the caller.
Traces: kms__lambda__s3.sse-kms-object-unreadable-when-key-policy-denies-role.

## Evidence
The check fails at its assertion that the SSE-KMS object is unreadable for the denied role. Expected: GetObject refused with HTTP 403 AccessDenied naming kms:Decrypt on the key ARN; observed: the read is not refused.

## Steps to Reproduce
CreateKey; CreateBucket; PutObject with ServerSideEncryption=aws:kms and SSEKMSKeyId=<key>; PutKeyPolicy with an explicit Deny of kms:Decrypt for the reading role; GetObject as that role.
