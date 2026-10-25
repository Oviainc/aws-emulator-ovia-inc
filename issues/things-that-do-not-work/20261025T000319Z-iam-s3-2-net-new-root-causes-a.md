Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__s3.presigned-get-signed-with-deleted-iam-user-key-rejected iam__s3.deleted-user-principal-rewrites-to-unique-id-in-bucket-policy
Issue-Key: iam__s3.presigned-get-signed-with-deleted-iam-user-key-rejected|run#30|AssertionError|assert error == expected, f"expected the S3 error {expected!r}, got {error!r} ({got[:300]!r})"
Issue-Key: iam__s3.deleted-user-principal-rewrites-to-unique-id-in-bucket-policy|_poll#10|Timeout|time.sleep(interval_s)
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# iam__s3: 2 net-new root causes — a request signed with a deleted IAM user's key is refused AccessDenied 'Sign the request with the emulator credentials' instead of InvalidAccessKeyId, and the deleted-principal bucket-policy rewrite never converges

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Two distinct net-new root causes, neither marked ALREADY FILED or OWNED in the replay report.

Cause 1 of 2: S3 refuses an unknown/retired access key with its own emulator-specific AccessDenied instead of S3's InvalidAccessKeyId.
AWS: a presigned GET signed with an access key that no longer exists (its IAM user was deleted) is refused by S3 with error code InvalidAccessKeyId and message 'The AWS Access Key Id you provided does not exist in our records.' — the code distinguishes 'this key is unknown to AWS' from 'this key exists but is not allowed'.
Emulator: answers code AccessDenied with the message 'Sign the request with the emulator credentials' (full body observed: <Error><Code>AccessDenied</Code><Message>Sign the request with the emulator credentials</Message>...), i.e. the credential gate collapses unknown-credential and authorization failures into one code and instructs the caller to use emulator-specific credentials.
Why: a client cannot tell a revoked/rotated credential (re-fetch credentials, do not retry as-is) from a permission problem (fix the policy); SDK credential-refresh logic keyed on InvalidAccessKeyId never triggers. The message also leaks an emulator-only concept into a response a client is expected to treat as S3's.
Fix: the request-signing/credential-resolution gate in front of the s3 service crate's handlers — resolve the SigV4 access key id against IAM's keys and, when it does not exist, answer S3's InvalidAccessKeyId (403) with AWS's message, reserving AccessDenied for authorization failures.
Traces: iam__s3.presigned-get-signed-with-deleted-iam-user-key-rejected.

Cause 2 of 2: the deleted-user principal rewrite in a bucket policy never materializes, so the check hangs to the 60 s cap.
AWS: when a bucket policy names an IAM user principal by ARN and that user is deleted, S3 rewrites the stored principal to the user's unique id (AIDA...) and GetBucketPolicy reports that id; the rewrite is observable by polling GetBucketPolicy after the delete.
Emulator: the check polls for that state change and hits 'Check timed out after 60.0s' — the stored principal is never rewritten (and the delete is not reflected in the policy at all).
Why: a client debugging a 'broken' bucket policy relies on the unique-id rewrite to see that the principal was deleted; without it the policy keeps naming an ARN that resolves to nothing, and tooling that detects orphaned principals cannot work.
Fix: on IAM DeleteUser, rewrite that principal's ARN to its unique id in stored S3 bucket policies (the bucket-policy store in the s3 service crate's state, driven from the iam crate's delete path).
Traces: iam__s3.deleted-user-principal-rewrites-to-unique-id-in-bucket-policy.

## Evidence
Cause 1: AssertionError: expected the S3 error ('InvalidAccessKeyId', 'The AWS Access Key Id you provided does not exist in our records.'), got ('AccessDenied', 'Sign the request with the emulator credentials') (b'<?xml version="1.0"?><Error><Code>AccessDenied</Code><Message>Sign the request with the emulator credentials</Message><RequestId>...</RequestId><HostId>...</HostId></Error>').
Cause 2: Error: Check timed out after 60.0s while polling GetBucketPolicy for the deleted principal's unique-id rewrite.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Cause 1: create an IAM user with an access key, presign a GetObject with that key, delete the user, then issue the presigned request — observe AccessDenied 'Sign the request with the emulator credentials' instead of InvalidAccessKeyId.
3. Cause 2: put a bucket policy whose Principal is that user's ARN, delete the user, then poll GetBucketPolicy for the principal rewritten to the user's unique id — it never changes.
4. Re-replay the two check ids in trace_ids to confirm.
