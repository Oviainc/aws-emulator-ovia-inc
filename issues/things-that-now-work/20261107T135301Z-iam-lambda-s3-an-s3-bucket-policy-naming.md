Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda__s3.bucket-policy-deleted-role-principal-rejected
Issue-Key: iam__lambda__s3.bucket-policy-deleted-role-principal-rejected|_invoke_lambda#10|Timeout|return result
Issue-Commit: 76f746b5965121353b22ae227b995a601e952faf

# iam__lambda__s3: an S3 bucket policy naming a deleted role principal is not refused and the check never settles

> _Note: this issue was identified against the code base at commit `76f746b5965121353b22ae227b995a601e952faf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketPolicy

## Description
One net-new root cause under '### iam__lambda__s3'; this check is not marked ALREADY FILED or OWNED (the open issue 20261105T085348Z-iam-lambda-s3-2-net-new-root-causes.md owns the other failing check in this service).

Cause 1 of 1: the deleted-role-principal bucket-policy probe never reaches an observable state.
AWS: PutBucketPolicy whose Statement names a role ARN that no longer exists is refused with 400 MalformedPolicy ('Invalid principal in policy'), because S3 resolves the principal at write time; the refusal is immediate, so the probe completes in one round trip.
Emulator: the check ends with 'Error: Check timed out after 60.0s' — neither a refusal nor an acceptance is produced within the budget.
Why: a client cannot discover that a policy references a dangling principal; the write neither succeeds nor fails observably, so IAM/S3 principal validation is unverifiable.
Fix: principal resolution/validation in the PutBucketPolicy handler (and whatever convergence loop leaves the request unanswered).
Traces: iam__lambda__s3.bucket-policy-deleted-role-principal-rejected.

## Evidence
'Error: Check timed out after 60.0s' for iam__lambda__s3.bucket-policy-deleted-role-principal-rejected; no status code or error code was produced to compare against AWS's 400 MalformedPolicy.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite.
2. Re-replay iam__lambda__s3.bucket-policy-deleted-role-principal-rejected (listed under '### iam__lambda__s3').
3. CreateRole, CreateBucket, DeleteRole, then PutBucketPolicy with Principal {'AWS': <deleted role arn>}.
