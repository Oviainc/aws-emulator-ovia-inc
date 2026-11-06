Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda__s3.bucket-policy-deleted-role-principal-rejected
Issue-Key: iam__lambda__s3.bucket-policy-deleted-role-principal-rejected|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# iam__lambda__s3: a bucket policy naming a deleted role principal never produces a verdict

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketPolicy

## Description
One root cause behind 1 failing check.

Cause 1 of 1: the scenario in which a bucket policy's `Principal` names an IAM role that has since been deleted never reaches an observable outcome.
AWS: PutBucketPolicy whose Principal is a role ARN that does not exist is refused immediately with HTTP 400 and error code `MalformedPolicy`, message `Invalid principal in policy` (fixed text; the offending principal is the value from the request). When the role existed at PutBucketPolicy time and is deleted afterwards, the policy survives but the deleted principal matches nothing, so access through it is refused with HTTP 403 `AccessDenied`. Either way an answer arrives in one call. Model: S3 PutBucketPolicy is modelled in services/service-com-amazonaws-s3-amazons3; the MalformedPolicy/`Invalid principal in policy` refusal is recorded behavior only.
Emulator: `Error: Check timed out after 60.0s` — the scenario yields no verdict at all, so neither the MalformedPolicy refusal nor the post-deletion denial is observable.
Why: a client writing bucket policies cannot learn that a principal is invalid; a stale policy referring to a deleted role is neither rejected nor shown to be inert, so an operator cannot reason about who can still reach the bucket.
Fix: the S3 PutBucketPolicy handler's principal validation and the bucket-policy evaluation path for a principal that no longer resolves. The open issue 20261105T085348Z-iam-lambda-s3-2-net-new-root-causes.md covers the absent in-runtime SDK and an incomplete `os` module for this service; it does not carry this replay key.
Traces: iam__lambda__s3.bucket-policy-deleted-role-principal-rejected.

## Evidence
Expected (AWS): PutBucketPolicy with a nonexistent role ARN as Principal -> HTTP 400 `MalformedPolicy`, message `Invalid principal in policy`; a policy whose principal role was deleted afterwards grants nothing (HTTP 403 AccessDenied through it).

Actual (emulator): iam__lambda__s3.bucket-policy-deleted-role-principal-rejected -> Error: Check timed out after 60.0s.

## Steps to Reproduce
1. Create a bucket and an IAM role; PutBucketPolicy with the role ARN as Principal.
2. Delete the role, then attempt access through that principal; and separately PutBucketPolicy naming a role ARN that never existed.
3. Expected: a prompt MalformedPolicy / AccessDenied answer. Observed: the check produces no verdict within 60 s.
