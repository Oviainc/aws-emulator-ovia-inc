Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3.bucket-policy-alone-grants-lambda-role-object-read lambda__s3.bucket-policy-deny-overrides-lambda-identity-allow
Issue-Key: lambda__s3.bucket-policy-alone-grants-lambda-role-object-read|run#13|AssertionError|assert before["granted"].get("code") == "AccessDenied", (
Issue-Key: lambda__s3.bucket-policy-deny-overrides-lambda-identity-allow|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# lambda__s3: in-function S3 calls raise UnknownServiceError, and a bucket-policy-deny check never settles (2 causes)

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Two root causes behind 2 failing checks.

Cause 1 of 2 (check: bucket-policy-alone-grants-lambda-role-object-read): the in-process Lambda SDK has no S3 service model, so the bucket-policy-versus-identity-policy decision is never reached.
AWS: a handler calling GetObject with a role that has NO s3 identity grant is refused with HTTP 403 and error code `AccessDenied`, message `User: arn:aws:sts::<account>:assumed-role/<role-name>/<function-name> is not authorized to perform: s3:GetObject on resource: "<object-arn>" because no identity-based policy allows the s3:GetObject action` (fixed text; the account, role, function and object ARN are request/role values) — unless the bucket policy itself grants that principal, in which case a same-account read is allowed by the resource policy alone. The distinction between the granted and the control role is exactly what must be observable. Model: S3 is present in this repo (services/service-com-amazonaws-s3-amazons3), so the client is constructible on the wire.
Emulator: BOTH the granted and the control invocation report `{'code': 'UnknownServiceError', 'message': "Unknown service: 's3'. Valid service names are: sqs, events, eventbridge, dynamodb"}` — the bundled botocore data catalog carries four services, so `boto3.client('s3')` fails before any request and the two roles are indistinguishable.
Why: no function can read or write S3, and bucket-policy-only authorization — the standard cross-principal grant — cannot be exercised or verified at all.
Fix: the in-process Lambda Python runtime's bundled AWS SDK service-model catalog / client factory, plus the S3 authorization path that must evaluate bucket policy and identity policy and answer AccessDenied when neither allows.

Cause 2 of 2 (check: bucket-policy-deny-overrides-lambda-identity-allow): the bucket-policy-deny scenario never reaches a verdict.
AWS: an explicit `Deny` in the bucket policy overrides any identity-based allow — GetObject is refused with HTTP 403 `AccessDenied` and no bytes are served. One call, one answer.
Emulator: `Error: Check timed out after 60.0s`.
Why: bucket-policy denies are the hard boundary on a bucket; if their precedence is unobservable, an operator cannot prove data is locked down.
Fix: the S3 authorization evaluator (explicit bucket-policy Deny must override an identity allow) and the 403 refusal path.
Traces: the two failing checks in trace_ids, under `### lambda__s3`. The open issue 20261105T111951Z-lambda-s3-the-lambda-python-runtime-has-no.md covers the absent SDK for other checks of this service and does not carry these replay keys.

## Evidence
Cause 1 — expected: the control role (no identity policy, no bucket-policy grant) gets HTTP 403 `AccessDenied` with the not-authorized message, while the bucket-policy-granted role reads the object.
Actual: AssertionError: a role with no s3 permission must be denied before the bucket policy: {'granted': {'code': 'UnknownServiceError', 'message': "Unknown service: 's3'. Valid service names are: sqs, events, eventbridge, dynamodb"}, 'control': {'code': 'UnknownServiceError', ...}}.

Cause 2 — expected: explicit bucket-policy Deny -> HTTP 403 AccessDenied despite the identity allow. Actual: Error: Check timed out after 60.0s.

## Steps to Reproduce
Cause 1: create a bucket with an object; role A has no s3 identity policy but is named in the bucket policy's Allow, role B has neither. Invoke a handler calling get_object under each. Expected: A reads, B gets 403 AccessDenied. Observed: both get UnknownServiceError.
Cause 2: grant s3:GetObject in the role's identity policy and add an explicit Deny for that principal in the bucket policy; invoke the handler. Expected 403 AccessDenied; observed no verdict within 60 s.
