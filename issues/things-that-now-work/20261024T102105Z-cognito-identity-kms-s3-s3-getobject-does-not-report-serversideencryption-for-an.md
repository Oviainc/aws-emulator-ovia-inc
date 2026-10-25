Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity__kms__s3.xsvc-cognito-identity-1-encryption-context-scoped-key-grant-denied-vended-session-outside-its-prefix
Issue-Key: cognito-identity__kms__s3.xsvc-cognito-identity-1-encryption-context-scoped-key-grant-denied-vended-session-outside-its-prefix|_poll_until#34|Timeout|time.sleep(interval_s)
Issue-Commit: aa31aa657622ba455ff8faedd6354f5956e3380b

# S3 GetObject does not report ServerSideEncryption for an SSE-KMS object, so the Cognito-vended-session encryption-context check never converges and hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `aa31aa657622ba455ff8faedd6354f5956e3380b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Cause 1 of 1 — GetObject's response omits the server-side-encryption indication that PutObject reported, so a reader cannot tell an SSE-KMS object from a plain one.
AWS: GetObject on an object written with ServerSideEncryption 'aws:kms' returns HTTP 200 with x-amz-server-side-encryption: aws:kms (surfaced as ServerSideEncryption in the response), alongside the object bytes; this is the same member PutObject echoes on write. The check uses exactly that member as its propagation barrier: it polls the read performed by a freshly vended Cognito identity-pool session until the response reports ServerSideEncryption == 'aws:kms', and only then asserts the negative leg (a key policy whose kms:EncryptionContext:aws:s3:arn condition names 'open/*' only must refuse the 'closed/' read with AccessDenied).
Emulator: the vended-session GetObject returns successfully (no ClientError is raised — the poller would have propagated one) but never reports ServerSideEncryption == 'aws:kms', so the predicate is false on every poll and the check ends as 'Check timed out after 60.0s'. Consequently the encryption-context-scoped key-grant assertions were never graded either; the check's PutObject legs did pass, reporting SSEKMSKeyId and BucketKeyEnabled=False correctly, which isolates the divergence to the READ response.
Why: any client that verifies an object's encryption on read — compliance scanners, SDK helpers that branch on ServerSideEncryption, and this whole class of prefix-scoped-decryption tests — sees SSE-KMS objects as unencrypted.
Fix: the GetObject response serializer must emit x-amz-server-side-encryption (and the related SSE-KMS members) for an object stored with SSE-KMS, matching what PutObject/HeadObject report.

## Evidence
cognito-identity__kms__s3.xsvc-cognito-identity-1-encryption-context-scoped-key-grant-denied-vended-session-outside-its-prefix: 'Error: Check timed out after 60.0s'. The hang is in _poll_until(lambda: _vended_s3().get_object(Bucket=bucket, Key='open/...'), lambda r: r.get('ServerSideEncryption') == 'aws:kms', timeout_s=180.0, interval_s=10.0, description='the context-conditioned grant to admit the open-prefix SSE-KMS read'); the probe raises nothing (a ClientError would propagate out of the poller), so GetObject answered 200 without ServerSideEncryption == 'aws:kms'. The earlier PutObject legs asserted successfully: put['SSEKMSKeyId'] == <key arn> and BucketKeyEnabled falsy. Expected read response: HTTP 200 with ServerSideEncryption 'aws:kms'.

## Steps to Reproduce
1. kms:CreateKey; s3:CreateBucket; PutObject(Key='open/o', ServerSideEncryption='aws:kms', SSEKMSKeyId=<key arn>, BucketKeyEnabled=False) — the response correctly reports SSEKMSKeyId. 2. s3:GetObject(Bucket, Key='open/o') and inspect ServerSideEncryption — absent in the emulator, 'aws:kms' in AWS. 3. (Blocked by 2) Create an identity pool with an authenticated role, add a key-policy statement allowing kms:Decrypt to that role conditioned on kms:EncryptionContext:aws:s3:arn like 'arn:aws:s3:::<bucket>/open/*', vend credentials, and confirm the 'open/' read succeeds while the 'closed/' read is AccessDenied.
