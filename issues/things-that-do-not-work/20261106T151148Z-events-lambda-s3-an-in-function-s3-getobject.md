Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__s3.lambda-role-missing-s3-permission-denied
Issue-Key: events__lambda__s3.lambda-role-missing-s3-permission-denied|run#8|AssertionError|assert allowed["denied"] is False and allowed["ok"] is True, (
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# events__lambda__s3: an in-function S3 GetObject with a granted role fails with an unclassified 'Unknown' error code

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
One root cause behind 1 failing check.

Cause 1 of 1: a handler whose execution role DOES grant `s3:GetObject` cannot read the object, and the failure carries the placeholder error code `Unknown` with no message.
AWS: with s3:GetObject granted on the object ARN, GetObject from inside the function returns HTTP 200 and the object body. Without the grant it is refused with HTTP 403 and error code `AccessDenied`, message `User: arn:aws:sts::<account>:assumed-role/<role-name>/<function-name> is not authorized to perform: s3:GetObject on resource: "<object-arn>" because no identity-based policy allows the s3:GetObject action` — fixed text with the account, role, function and object ARN taken from the request and the role. Either way the answer carries a real S3 error code, never `Unknown`, and never a null message. Model: S3 GetObject is modelled in services/service-com-amazonaws-s3-amazons3 (NoSuchKey, NoSuchBucket, InvalidObjectState); the authorization refusal is AccessDenied.
Emulator: the positive control fails — `{'ok': False, 'denied': True, 'error_code': 'Unknown', 'message': None}`. The granted read is refused, the error code is the placeholder `Unknown` (not an AWS code), and the message is absent. Because the positive control cannot pass, the negative half proves nothing either.
Why: a client cannot read its own objects, and a caller that branches on `e.response['Error']['Code']` sees a code AWS never emits with no message to log — the failure is undiagnosable and no retry/fallback policy can key on it.
Fix: the in-function AWS SDK call path for S3 and the error serializer behind it: a refusal must carry S3's own code (`AccessDenied`, `NoSuchKey`, ...) and its message, never `Unknown`/null. The open issue 20261105T094343Z-events-lambda-s3-the-lambda-python-runtime-provides.md records the absent in-runtime SDK for other checks of this service; it does not carry this replay key.
Traces: events__lambda__s3.lambda-role-missing-s3-permission-denied.

## Evidence
Expected (AWS): with s3:GetObject granted the handler reads the object (HTTP 200); without it, HTTP 403 Error.Code `AccessDenied` with the `... is not authorized to perform: s3:GetObject on resource: "<arn>" because no identity-based policy allows the s3:GetObject action` message.

Actual (emulator): AssertionError: positive control: role granted s3:GetObject should succeed, got {'ok': False, 'denied': True, 'error_code': 'Unknown', 'message': None}.

## Steps to Reproduce
1. Create a bucket with an object and a Lambda execution role granting s3:GetObject on that object ARN.
2. Invoke a handler that calls boto3.client('s3').get_object(...) and reports the outcome.
3. Expected: the body is returned. Observed: ok=False with error_code 'Unknown' and message None.
