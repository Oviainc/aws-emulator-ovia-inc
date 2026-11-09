Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__s3.obs-s3-2-object-data-events-record-success-missing-key-and-denied-calls
Issue-Key: cloudtrail__s3.obs-s3-2-object-data-events-record-success-missing-key-and-denied-calls|_error#36|AssertionError|raise AssertionError("expected the call to be rejected, but it succeeded")
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# S3 GetObject succeeds for a role whose policy denies it (object-level authorization is never evaluated)

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
There is 1 cause in this issue.

Cause 1 of 1 — S3 object reads are not authorized against the caller's identity.
AWS: recorded behavior only: GetObject issued by an assumed role whose effective policy does not permit s3:GetObject on that key is refused with error code AccessDenied and HTTP 403; the object bytes are not returned, and CloudTrail records the call with that errorCode. The check assumes a role built for exactly this purpose and reads a key it is denied.
Emulator: the GetObject returned the object successfully — the check raised 'expected the call to be rejected, but it succeeded' from its _error helper, so no AccessDenied was produced and no 403 was seen.
Why: this is an over-permissive divergence, the most dangerous kind. A client's least-privilege test suite passes against the emulator and fails on AWS; object data that should be unreachable is served; and no denied-call audit record exists to assert against, so the entire denied-access branch of a client's error handling and of its audit pipeline is dead.
Fix: the request-authorization layer for S3 object operations must evaluate the caller's identity policy and the bucket policy and refuse with AccessDenied (403) before the object handler reads state.
Traces: cloudtrail__s3.obs-s3-2-object-data-events-record-success-missing-key-and-denied-calls.

The check id carries the composite service token 'cloudtrail__s3', which the per-service bootstrap gate cannot place by exact equality; both cloudtrail and s3 are stood up and neither appears in the pending set, so this is filed rather than deferred. The open issue 20261022T220446Z-iam-no-iam-authorization-is-evaluated-calls-a-session.md records the absence of authorization on the IAM side and owns other checks in this suite; it does not carry this check's key, and this cause is specifically S3 object-level authorization.

## Evidence
File "check_cloudtrail__s3_obs_s3_2_object_data_events_record_success_missing_key_and_denied_calls.py", line 437, in _round -> code, message = _error(lambda: s3_role.get_object(Bucket=data_bucket, Key=keys["denied_get"])) -> line 44 -> AssertionError: expected the call to be rejected, but it succeeded. Expected: error code AccessDenied, HTTP 403, no object body.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Replay cloudtrail__s3.obs-s3-2-object-data-events-record-success-missing-key-and-denied-calls.
3. Manually: create a bucket and put an object; create an IAM role whose inline policy does not grant s3:GetObject on that key; assume it with sts; call s3 GetObject with those credentials -> expect AccessDenied 403, observe HTTP 200 with the object body.
