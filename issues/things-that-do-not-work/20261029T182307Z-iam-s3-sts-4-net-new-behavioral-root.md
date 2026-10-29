Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__s3__sts.assumed-role-permissions-policy-requestedregion-condition-gates-put-object-by-region iam__s3__sts.xsvc-iam-2-s3-same-name-user-arn-principal-does-not-admit-role-session-denied iam__s3__sts.xsvc-iam-2-s3-session-arn-principal-admits-only-that-session-denied iam__s3__sts.s3-sts-session-credentials-object-roundtrip
Issue-Key: iam__s3__sts.assumed-role-permissions-policy-requestedregion-condition-gates-put-object-by-region|_poll#50|Timeout|time.sleep(interval_s)
Issue-Key: iam__s3__sts.xsvc-iam-2-s3-same-name-user-arn-principal-does-not-admit-role-session-denied|_settle#24|Timeout|time.sleep(interval_s)
Issue-Key: iam__s3__sts.xsvc-iam-2-s3-session-arn-principal-admits-only-that-session-denied|_settle#26|Timeout|time.sleep(interval_s)
Issue-Key: iam__s3__sts|PutObject|NoSuchBucket
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# iam__s3__sts: 4 net-new behavioral root causes — S3 ignores aws:RequestedRegion in a role's own policy, bucket-policy Principal matching admits a role session for a same-named user ARN and any session for a session ARN, and NoSuchBucket omits BucketName

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
Four net-new root causes on the S3 data plane when the caller is an STS session. They are numbered so a fixer can close them one at a time; all four must be closed before submitting.

Related but NOT covering these: open issue 20261024T065715Z-iam-s3-sts-s3-requests-under-a-scoped-session-are-never.md (which the suite reports as owning only iam__s3__sts.xsvc-iam-2-s3-role-arn-principal-authorizes-every-session) and open issue 20261029T065036Z-iam-s3-sts-an-explicit-deny-conditioned-on-aws-multifactorauthpresent-is.md. Neither carries the four replay keys below. Causes 1-3 each exercise a DIFFERENT input to the S3 authorization decision — a condition key derived from the request's region, and two Principal FORMS in a resource policy — so a gate wired for one of them still gets the other two wrong. Cause 4 is unrelated to authorization: it is an error-serialization defect.

=== Cause 1 of 4: an aws:RequestedRegion condition in the ROLE's OWN permissions policy is not evaluated, so one session reaches every region ===
AWS: documented IAM behavior (aws:RequestedRegion is a global condition key DERIVED from the region the request is actually sent to, not carried in the request body); the Smithy model for s3#PutObject states no condition keys, so recorded behavior only for the outcome. A role whose own identity-based permissions policy Allows s3:PutObject only under `StringEquals aws:RequestedRegion == <region A>` yields a session whose PutObject SUCCEEDS against a bucket in region A and is refused with error code `AccessDenied` (HTTP 403) against an equivalent bucket in region B — same credentials, no session policy involved. A control session assumed from the identical role with its permissions policy temporarily replaced by an unconditional one reaches BOTH regions, which is what proves the refusal belongs to the condition and not to the bucket or the region.
Emulator: the refusal never arrives. The check establishes the positive control, then polls for the AccessDenied in region B and exhausts the harness cap (`Error: Check timed out after 60.0s`). No divergent response body is captured because the emulator never produces one — PutObject keeps answering 200 in both regions.
Why: every region-fencing policy is inert. A client's data-residency guardrail written as aws:RequestedRegion validates clean against the emulator and then does nothing, and conversely no test can observe that a cross-region write was supposed to be refused. Note this is a materially different code path from a session-policy RequestedRegion condition passed in AssumeRole's `Policy=` parameter: here the condition lives in the role's own identity-based document with no session policy at all, so the key must be derived identically on that path.
Fix: the S3 request-authorization gate — S3's PutObject handler must derive the (action, resource) pair and the aws:RequestedRegion value from the region of the request it is serving and ask the shared evaluation (services/common/src/iam_authorization.rs) for a decision against the role's identity-based documents.
Traces: iam__s3__sts.assumed-role-permissions-policy-requestedregion-condition-gates-put-object-by-region

=== Cause 2 of 4: a bucket-policy Principal naming an IAM USER arn admits a ROLE session that merely shares the name ===
AWS: documented IAM principal-matching behavior; the Smithy model states nothing about resource policies, so recorded behavior only for the outcome. `arn:aws:iam::<account>:user/<name>` and `arn:aws:iam::<account>:role/<name>` are DIFFERENT principals even when `<name>` is identical, and a bucket policy whose only Allow names the user arn grants nothing to a session assumed from the same-named role. The S3 call made with that session's credentials is refused with error code `AccessDenied` (HTTP 403).
Emulator: the refusal never arrives — the check's bounded poll for the denial exhausts the harness cap (`Error: Check timed out after 60.0s`); the call under the role session keeps succeeding against a bucket policy that names only the user.
Why: resource-policy principal matching is the whole basis of cross-account and cross-principal S3 sharing. Collapsing `user/<name>` and `role/<name>` grants data to a principal AWS refuses, and it is silent — nothing in the response says the grant was widened.
Fix: the principal-matching step of S3's resource-policy evaluation — the Principal entry must be compared as a full ARN including its resource TYPE segment, not by account plus trailing name.
Traces: iam__s3__sts.xsvc-iam-2-s3-same-name-user-arn-principal-does-not-admit-role-session-denied

=== Cause 3 of 4: a bucket-policy Principal naming ONE assumed-role SESSION arn admits other sessions ===
AWS: documented IAM principal-matching behavior; recorded behavior only for the outcome. A bucket policy whose only Allow names a specific assumed-role session principal `arn:aws:sts::<account>:assumed-role/<role>/<sessionName>` admits ONLY that session; an otherwise identical session of the same role minted under a different RoleSessionName is refused with error code `AccessDenied` (HTTP 403).
Emulator: that refusal never arrives either — `Error: Check timed out after 60.0s` while polling for the denial of the non-matching session.
Why: a grant written to pin access to one named session is the standard way to scope a short-lived delegation, and here it is honoured for every session of the role. Together with cause 2 this means the Principal element's specificity is not enforced at all on S3, in two independent directions.
Fix: the same principal-matching step as cause 2, extended to the assumed-role session form — the `/<sessionName>` segment must participate in the comparison rather than being truncated to the role.
Traces: iam__s3__sts.xsvc-iam-2-s3-session-arn-principal-admits-only-that-session-denied

=== Cause 4 of 4: the NoSuchBucket error PutObject returns carries no BucketName ===
AWS: Smithy (com.amazonaws.s3#NoSuchBucket): errorKind client, httpError 404, doc "The specified bucket does not exist." — the modelled shape declares no members, so the BucketName element is recorded behavior only. S3's REST-XML error document for a PutObject against a bucket that does not exist carries, in addition to Code/Message, a `<BucketName>` element whose value is the bucket NAMED IN THE REQUEST, which botocore surfaces as `response['Error']['BucketName']`. The fixed parts are the element name and `Code` = `NoSuchBucket`; the variable part is the bucket name taken from the request URI.
Emulator: the 404 NoSuchBucket is produced with the element absent — `response['Error'].get('BucketName')` is None. Replay: `AssertionError: PutObject NoSuchBucket BucketName: expected 'sim2real-s3sts-<id>', got None` (the bucket name shown is a per-run generated name; the requirement is that the element echo whatever bucket the request named).
Why: tooling that reports or retries on which bucket was missing — the AWS CLI's own messages, and any client that disambiguates a multi-bucket operation from the error alone — has nothing to read, and a client cannot tell which of several buckets in a pipeline is the missing one.
Fix: the S3 error serializer for NoSuchBucket (the REST-XML error-document writer), which must emit the extra `BucketName` element alongside Code and Message; the handler already knows the requested bucket.
Traces: iam__s3__sts.s3-sts-session-credentials-object-roundtrip

## Evidence
Cause 1 — iam__s3__sts.assumed-role-permissions-policy-requestedregion-condition-gates-put-object-by-region, [mismatch]:
  Error: Check timed out after 60.0s
  The check asserts PutObject SUCCEEDS in the permitted region and returns error code 'AccessDenied' in the second region under the SAME session credentials, with an unconditional-policy control session reaching both. The emulator never produces the AccessDenied, so the bounded poll runs out the harness budget.

Cause 2 — iam__s3__sts.xsvc-iam-2-s3-same-name-user-arn-principal-does-not-admit-role-session-denied, [mismatch]:
  Error: Check timed out after 60.0s
  Expected: error code 'AccessDenied' for the role-session call against a bucket policy whose only Allow Principal is arn:aws:iam::<account>:user/<name>. Actual: the call keeps succeeding ('OK').

Cause 3 — iam__s3__sts.xsvc-iam-2-s3-session-arn-principal-admits-only-that-session-denied, [mismatch]:
  Error: Check timed out after 60.0s
  Expected: error code 'AccessDenied' for a DIFFERENT session of the same role against a bucket policy whose only Allow Principal is arn:aws:sts::<account>:assumed-role/<role>/<sessionName>. Actual: the call keeps succeeding ('OK').

Cause 4 — iam__s3__sts.s3-sts-session-credentials-object-roundtrip, [mismatch], at check line 259:
  assert named == bucket, f"PutObject NoSuchBucket BucketName: expected {bucket!r}, got {named!r}"
  AssertionError: PutObject NoSuchBucket BucketName: expected 'sim2real-s3sts-e5ddbe7b7b6e', got None
  Status 404 and Code 'NoSuchBucket' are correct; the BucketName element is missing from the error document.

## Steps to Reproduce
1. Boot the emulator and replay the four check ids in trace_ids.
2. Cause 1: create a role assumable by the caller whose inline permissions policy Allows s3:PutObject on both of two buckets but only under Condition {StringEquals: {'aws:RequestedRegion': '<region A>'}}. Create bucket A in region A and bucket B in a second region (never us-east-1). sts.assume_role, then PutObject with the session credentials through a region-A-pinned client (expect 200) and through a region-B-pinned client (AWS: AccessDenied/403; emulator: 200). Replace the role policy with an unconditional Allow to confirm both regions are otherwise reachable.
3. Cause 2: create IAM user `X` and role `X` (same name), put a bucket policy whose only statement Allows s3:GetObject with Principal {'AWS': 'arn:aws:iam::<acct>:user/X'}, assume role `X` and GetObject with the session credentials. AWS refuses AccessDenied; the emulator serves the object.
4. Cause 3: put a bucket policy whose only statement Allows s3:GetObject with Principal {'AWS': 'arn:aws:sts::<acct>:assumed-role/X/session-one'}, then assume role `X` with RoleSessionName='session-two' and GetObject. AWS refuses AccessDenied; the emulator serves the object.
5. Cause 4: create a bucket, delete it, then s3.put_object(Bucket=<that bucket>, Key='k', Body=b'x') and inspect the raised ClientError: `exc.response['Error']['BucketName']` must equal the bucket named in the request; the emulator omits the key entirely.
