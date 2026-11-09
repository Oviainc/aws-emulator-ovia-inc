Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__s3.resource-policy-alone-authorizes-a-principal-with-no-identity-policy
Issue-Key: iam__s3.resource-policy-alone-authorizes-a-principal-with-no-identity-policy|_rejected_error#23|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Commit: f5ba8024c64455531a4bd3d9779b46ec0fe104e4

# iam__s3: S3 GetObject is authorized with neither an identity-policy nor a bucket-policy grant — the no-grant state returns the object instead of AccessDenied

> _Note: this issue was identified against the code base at commit `f5ba8024c64455531a4bd3d9779b46ec0fe104e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Cause 1 of 1: S3's object read performs no same-account authorization, so a principal with no grant at all reads the object.
AWS: same-account access to an object is the union of the caller's identity policies and the bucket policy. With NEITHER grant present, s3:GetObject is refused with error code AccessDenied (HTTP 403); a grant in either place alone suffices to allow it. (Recorded behavior only for the exact refusal: the check records the refusal code as AccessDenied on GetObject; the Smithy model declares no modelled error for this condition, S3 returns the generic AccessDenied.)
Emulator: GetObject answers HTTP 200 and returns the object body (ContentLength 38, ETag present, x-amz-request-id set) for an IAM user holding no identity-policy grant and against a bucket whose policy does not name it.
Why: every S3 least-privilege boundary is unenforced. A principal that AWS refuses reads object data, so an application's access-control model is silently void and no negative control on S3 authorization can ever fail. This is a confidentiality divergence, not a cosmetic one.
Fix: S3's request authorization layer for the object data plane — it must evaluate the caller's identity policies and the bucket policy and refuse with AccessDenied when neither grants the action. (The open issue 20261027T000203Z-iam-s3-s3-getobject-performs-no-iam-authorization-a-principal.md records the same absence for a different scenario and owns a different check; it does not carry this replay key, so it does not cover this one.)
Traces: iam__s3.resource-policy-alone-authorizes-a-principal-with-no-identity-policy

## Evidence
Expected: GetObject refused with AccessDenied in the no-grant state. Actual: `real AWS refuses this call, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'AcceptRanges': 'bytes', 'ContentLength': 38, 'ETag': '"f7fc0a43eb3b1bd01f6f9ee2c360bb52"', 'ContentType': 'binary/octet-stream', 'ServerSideEncryption': 'AES256', 'Body': <StreamingChecksumBody>}`. The ETag, request id and length are per-run values; the requirement is the 403 AccessDenied refusal.

## Steps to Reproduce
Create a bucket and put an object in it. Create an IAM user with an access key and NO identity-policy grant for s3:GetObject, and do not put a bucket policy naming that user. Signed as that user, call s3:GetObject on the object. AWS refuses with 403 AccessDenied; the emulator returns 200 and the object body. (The same check then verifies that a bucket-policy-only grant, and separately an identity-policy-only grant, each allow the read — so the fix must preserve the union semantics.)
