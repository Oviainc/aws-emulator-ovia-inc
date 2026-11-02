Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__iam__sts.ddb-rbp-deleted-iam-user-principal-is-served-as-its-unique-id
Issue-Key: dynamodb__iam__sts.ddb-rbp-deleted-iam-user-principal-is-served-as-its-unique-id|_poll#14|Timeout|time.sleep(interval_s)
Issue-Commit: 3879dd05c1f9087ace2750498c91b002c8086d49

# dynamodb: a resource-based policy principal that is a deleted IAM user is never served as its unique id

> _Note: this issue was identified against the code base at commit `3879dd05c1f9087ace2750498c91b002c8086d49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePolicy

## Description
Cause 1 of 1: a table resource policy that names an IAM user ARN is not rewritten when that user is deleted.
AWS: an identity named in a resource-based policy is stored by its unique id, so after the IAM user is deleted the policy read back through GetResourcePolicy serves the principal as that unique id (an AIDA-prefixed id) rather than the now-dangling user ARN.
Emulator: that state is never reached — the check's bounded poll for the rewritten principal never converges and the check exhausts the 60 s budget, so the policy keeps whatever was written and the deletion is not reflected.
Why: an operator auditing a table policy cannot see that a named principal no longer exists, and a policy that AWS would have neutralized still reads as a live grant to a user ARN that could be recreated.
Fix: the resource-policy store behind PutResourcePolicy/GetResourcePolicy must resolve identity principals to their unique ids and surface the deleted-principal form. The analogous S3 behavior is filed as 20261025T000319Z-iam-s3-2-net-new-root-causes-a.md, which carries only that service's checks.
Traces: dynamodb__iam__sts.ddb-rbp-deleted-iam-user-principal-is-served-as-its-unique-id.

## Evidence
'Error: Check timed out after 60.0s' — the poll for the resource policy to serve the deleted user's unique id never converges.

## Steps to Reproduce
1. Create a table, PutResourcePolicy naming an IAM user's ARN as Principal.
2. Delete that IAM user, then GetResourcePolicy and observe the principal is never served as the user's unique id.
3. Or replay dynamodb__iam__sts.ddb-rbp-deleted-iam-user-principal-is-served-as-its-unique-id.
