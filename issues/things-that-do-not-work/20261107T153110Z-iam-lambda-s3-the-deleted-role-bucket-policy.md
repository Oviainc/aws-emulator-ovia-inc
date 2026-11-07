Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda__s3.bucket-policy-deleted-role-principal-rejected
Issue-Key: iam__lambda__s3.bucket-policy-deleted-role-principal-rejected|_invoke_lambda#10|Timeout|resp = ctx.client("lambda").invoke(
Issue-Commit: ef1bb1e3a86f6fb50b04ae6c71e141a327380e0c

# iam__lambda__s3: the deleted-role bucket-policy principal check never converges — PutBucketPolicy/Invoke round trip exhausts the whole 60s check budget

> _Note: this issue was identified against the code base at commit `ef1bb1e3a86f6fb50b04ae6c71e141a327380e0c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketPolicy

## Description
One net-new root cause. Cause count: 1. A fixer must close this one cause before submitting.

Not marked "ALREADY FILED" or "OWNED by" in this replay report. Related but NOT covering, named here so a fixer can see the history: open 20261105T085348Z-iam-lambda-s3-2-net-new-root-causes.md (Cause 1: the in-process python Lambda runtime ships no boto3, so an `import boto3` handler faults with Runtime.ImportModuleError) lists this check id among its Issue-Trace-Ids, but its replay key is an AssertionError raised inside `_invoke_payload` ('handler faulted instead of returning a result'). In THIS run the check no longer reaches that assertion at all: it produces no assertion, no AWS error code and no response — it is killed by the 60s per-check budget. The observable has therefore changed, which is why the report does not treat that issue as owning this key; a fixer closing only the boto3 import gap would not necessarily make this check complete.

=== Cause 1 of 1: the live-principal bucket policy / Lambda read round trip never reaches a steady state, so the deleted-role principal rejection is never exercised ===
AWS: S3 is restXml. Smithy: PutBucketPolicy is PUT /{Bucket}?policy with Bucket as an @httpLabel @required and Policy as the @httpPayload @required member, success 200 and no modeled errors (its refusals are the service-common restXml errors); GetBucketPolicy is GET /{Bucket}?policy returning the Policy document as the @httpPayload. Recorded behavior only, for the sequence this check drives: (a) PutBucketPolicy naming a LIVE IAM role ARN as the statement's `Principal.AWS` is accepted with 200 and that statement then authorizes the role's s3:GetObject, so a Lambda running under that execution role reads the object and returns its body; (b) PutBucketPolicy naming a role ARN that EXISTED and was then deleted is refused — HTTP 400, Error.Code `MalformedPolicy`, Error.Message exactly `Invalid principal in policy` (the whole message is fixed text; it carries no request-derived value); (c) the refused put does not mutate the bucket policy, so GetBucketPolicy still returns the previously accepted document and the Lambda read still succeeds; (d) recreating a role under the IDENTICAL name makes the byte-identical PutBucketPolicy succeed, proving the refusal was attributable to the principal's nonexistence alone and not to the ARN string, a cache or a denylist.
Emulator: the check never gets far enough to report on any of (b)-(d). Its only output is `Error: Check timed out after 60.0s` — no HTTP status, no error code, no payload. Every step before the first long wait is a loop that only terminates on the emulator reaching an AWS-consistent state: `_create_function_with_retry` (CreateFunction), `_retry_client_error(PutBucketPolicy naming the live execution role, codes=('MalformedPolicy','InvalidArgument'))`, and `_poll_until(Invoke → payload contains 'body', timeout_s=120)`. One of those three never converges within the budget. Hypothesis (neither the model nor the recording states which): either PutBucketPolicy persistently refuses a policy whose principal is a live role ARN with MalformedPolicy/InvalidArgument — i.e. the principal is resolved wrongly in the accept direction as well — or the Invoke of the boto3-importing handler never returns a payload containing `body`, which is the direction the related open issue describes.
Why: a client cannot rely on the emulator for bucket-policy principal validation at all. Everything the check exists to prove is unobservable: a dangling (deleted-role) principal may be silently ACCEPTED, which is the security-relevant direction — a policy AWS rejects outright would appear to be in force — and a rejected put may or may not have replaced the policy already in force. Because the operation hangs rather than answering, a caller also sees no error to handle: a tool that would retry or surface MalformedPolicy instead blocks until its own timeout.
Fix: the PutBucketPolicy handler's policy-document validation in the S3 crate (the component that resolves a statement's `Principal.AWS` ARN against IAM state and must refuse an unresolvable principal with MalformedPolicy / `Invalid principal in policy` while accepting a live role ARN, leaving the stored policy untouched on refusal), together with the lambda Invoke execution path the check reads the authorization effect through. Each step must answer within one request rather than leaving the caller polling: the accept leg must return 200 for a live role ARN, and Invoke must return the handler's own payload.
Traces: iam__lambda__s3.bucket-policy-deleted-role-principal-rejected (run(), the `_retry_client_error`/`_poll_until` sequence at check lines 2710-2722 of the embedded source — 'PutBucketPolicy naming the live Lambda execution role (IAM propagation)' and 'the live-principal bucket policy authorizes the Lambda read'). The sibling check iam__lambda__s3.e2e-csv-to-xlsx-pipeline-lifecycle in the same group is OWNED by the related issue above and is NOT part of this report.

## Evidence
Suite report, `### iam__lambda__s3` (per-service 0/2, 2 failed):

  [mismatch] iam__lambda__s3.bucket-policy-deleted-role-principal-rejected
  Error: Check timed out after 60.0s

That is the entire detail the replay produced — the check was killed before any of its assertions ran, so no error code, status or payload was captured. Expected, from the check's own docstring and asserts: PutBucketPolicy naming the live Lambda execution role returns 200; the Lambda invoke returns {'body': '<object bytes>'}; PutBucketPolicy naming the deleted role's ARN returns MalformedPolicy with message exactly 'Invalid principal in policy'; GetBucketPolicy still shows the statement Sid 'AllowLiveLambdaRole'; after recreating the role under the identical name the same PutBucketPolicy succeeds and GetBucketPolicy shows Sid 'AllowDeletedRole'.

Actual: none of these were reached. The three convergence loops executed before the first assertion are, from the embedded check source: `_create_function_with_retry(...)`; `_retry_client_error(lambda: s3.put_bucket_policy(Bucket=bucket, Policy=json.dumps(live_policy)), codes=('MalformedPolicy','InvalidArgument'), description='PutBucketPolicy naming the live Lambda execution role (IAM propagation)')`; and `_poll_until(lambda: _invoke_payload(ctx, fn['name'], {}), lambda r: 'body' in r, timeout_s=120.0, interval_s=5.0, description='the live-principal bucket policy authorizes the Lambda read')`. The check's own poll budget (120s) exceeds the harness budget (60s), so a non-converging emulator state surfaces as this timeout rather than as the poll's own message.

The failure is attributed to the emulator, not to the runner: no tool was missing, no TLS or name-resolution failure was reported, and no account-specific or live-only value is asserted anywhere in the check — every identifier it uses (bucket name, role name, function name) it creates itself.

## Steps to Reproduce
Build and boot the emulator from this checkout and replay the check in trace_ids; the timeout reproduces. To localize which leg hangs, drive the sequence by hand against the s3, iam and lambda endpoints:

1. s3 CreateBucket(Bucket=<b>) (with CreateBucketConfiguration.LocationConstraint when the region is not us-east-1), PutBucketTagging, and PutObject(Bucket=<b>, Key='data.txt', Body=<bytes>).
2. iam CreateRole(RoleName=<live>, Path=/sim2real/) trusting lambda.amazonaws.com, and lambda CreateFunction(FunctionName=<f>, Role=<live's ARN>) with a python handler that does `import boto3` and returns {'body': s3.get_object(Bucket=os.environ['BUCKET'], Key='data.txt')['Body'].read().decode()}.
3. s3 PutBucketPolicy(Bucket=<b>, Policy=<one Allow statement, Sid 'AllowLiveLambdaRole', Principal.AWS = <live's ARN>, Action s3:GetObject, Resource arn:aws:s3:::<b>/*>). Expect 200. If this answers MalformedPolicy or InvalidArgument repeatedly, leg 1 is the cause.
4. lambda Invoke(FunctionName=<f>, InvocationType='RequestResponse'). Expect StatusCode 200, no X-Amz-Function-Error, and a payload containing 'body' equal to the object's bytes. If the payload never contains 'body', leg 2 is the cause.
5. iam CreateRole(RoleName=<doomed>) then iam DeleteRole(RoleName=<doomed>), and poll iam GetRole(RoleName=<doomed>) until it answers NoSuchEntity.
6. s3 PutBucketPolicy(Bucket=<b>, Policy=<the same statement with Sid 'AllowDeletedRole' and Principal.AWS = <doomed's ARN>>). Expect HTTP 400 / Error.Code 'MalformedPolicy' / Error.Message exactly 'Invalid principal in policy'.
7. s3 GetBucketPolicy(Bucket=<b>): the stored document must still be the one from step 3 (Sid 'AllowLiveLambdaRole'), and Invoke must still return the object body.
8. iam CreateRole(RoleName=<doomed>) again with the identical name, then repeat step 6's call verbatim: it must now succeed, and GetBucketPolicy must show Sid 'AllowDeletedRole'.
