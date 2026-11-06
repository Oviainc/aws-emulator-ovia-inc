Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__s3.cfn-update-iam-role-policy-reflected-in-lambda-execution-positive cloudformation__iam__lambda__s3.iam-explicit-deny-overrides-allow-s3-negative cloudformation__iam__lambda__s3.iam-role-policy-update-reflected-lambda-s3-access-positive cloudformation__iam__lambda__s3.lambda-role-permission-boundary-restricts-s3-negative cloudformation__iam__lambda__s3.lambda-role-s3-resource-arn-mismatch-negative cloudformation__iam__lambda__s3.iam-role-trust-policy-allows-lambda-assume-positive cloudformation__iam__lambda__s3.lambda-role-wildcard-resource-s3-access-positive
Issue-Key: cloudformation__iam__lambda__s3.cfn-update-iam-role-policy-reflected-in-lambda-execution-positive|run#14|AssertionError|assert before["error"] in ("403", "AccessDenied"), (
Issue-Key: cloudformation__iam__lambda__s3.iam-explicit-deny-overrides-allow-s3-negative|run#12|AssertionError|assert denied["error"] == "AccessDenied", (
Issue-Key: cloudformation__iam__lambda__s3.iam-role-policy-update-reflected-lambda-s3-access-positive|run#10|AssertionError|assert baseline["error"] == "AccessDenied"
Issue-Key: cloudformation__iam__lambda__s3.lambda-role-permission-boundary-restricts-s3-negative|run#12|AssertionError|assert denied["error"] == "AccessDenied", (
Issue-Key: cloudformation__iam__lambda__s3.lambda-role-s3-resource-arn-mismatch-negative|run#12|AssertionError|assert denied["error"] == "AccessDenied", (
Issue-Key: cloudformation__iam__lambda__s3.iam-role-trust-policy-allows-lambda-assume-positive|_invoke#9|Timeout|resp = lam.invoke(FunctionName=name, Payload=json.dumps(payload).encode("utf-8"))
Issue-Key: cloudformation__iam__lambda__s3.lambda-role-wildcard-resource-s3-access-positive|_invoke#8|Timeout|resp = lam.invoke(FunctionName=name, Payload=json.dumps(payload).encode("utf-8"))
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# cloudformation__iam__lambda__s3: in-function S3 calls raise UnknownServiceError so no IAM decision on S3 is observable (2 causes)

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
Two root causes behind 7 failing checks.

Cause 1 of 2 (5 checks: cfn-update-iam-role-policy-reflected-in-lambda-execution-positive, iam-explicit-deny-overrides-allow-s3-negative, iam-role-policy-update-reflected-lambda-s3-access-positive, lambda-role-permission-boundary-restricts-s3-negative, lambda-role-s3-resource-arn-mismatch-negative): the in-process Lambda SDK has no S3 service model, so neither the allow nor the deny decision AWS would make is ever reached.
AWS: a handler calling `boto3.client('s3').put_object(...)`/`get_object(...)` either succeeds, or is refused with HTTP 403 and error code `AccessDenied` whose message is the fixed `User: arn:aws:sts::<account>:assumed-role/<role-name>/<function-name> is not authorized to perform: s3:PutObject on resource: "<object-arn>" because no identity-based policy allows the s3:PutObject action` — and `because no permissions boundary allows ...` / `with an explicit deny in an identity-based policy` when a boundary or an explicit Deny is what refuses it. Account, role, function, action and resource ARN are values from the request and the role; the clause naming WHICH policy type refused is itself a requirement, because that is how an operator finds the policy. Model: S3 is present in this repo (services/service-com-amazonaws-s3-amazons3).
Emulator: every one of these handlers raises errorType `UnknownServiceError`, errorMessage `Unknown service: 's3'. Valid service names are: sqs, events, eventbridge, dynamodb` — the bundled data catalog carries four services, so `boto3.client('s3')` fails before any request. The checks therefore see `'UnknownServiceError'` where they expect 403/AccessDenied, and the positive controls fail too.
Why: an explicit Deny, a permission boundary and a wrong-resource-ARN policy are indistinguishable from a broken SDK; identity-policy changes can never be shown to take effect; and no function can touch S3 at all.
Fix: the in-process Lambda Python runtime's bundled AWS SDK service-model catalog and client factory, plus the IAM evaluation path that must classify the refusal (identity policy / explicit deny / permissions boundary) in the error message. Open issue 20261105T085348Z-cloudformation-iam-lambda-s3-2-net-new-root.md names the absent SDK for other checks of this service but does not carry these replay keys.

Cause 2 of 2 (2 checks: iam-role-trust-policy-allows-lambda-assume-positive, lambda-role-wildcard-resource-s3-access-positive): the scenario never reaches a verdict — the check exhausts its whole 60 s budget.
AWS: a role whose trust policy allows lambda.amazonaws.com is assumable by the function and a wildcard-resource s3:* identity policy lets the handler write any object; both settle in seconds.
Emulator: `Error: Check timed out after 60.0s` — no outcome is ever produced, so neither the assume nor the wildcard grant is observable.
Why: a client cannot tell whether a wildcard grant works, and a trust-policy change can never be confirmed.
Fix: the Lambda invoke/execution path for role-assumption and the S3 authorization path it drives; whatever it waits on must settle.
Traces: the 7 failing checks in trace_ids, under `### cloudformation__iam__lambda__s3`.

## Evidence
Cause 1 — expected (AWS): `AssertionError`-free run; the denied variants surface HTTP 403 `AccessDenied` with `... is not authorized to perform: s3:PutObject on resource: "<arn>" because no identity-based policy allows the s3:PutObject action` (or the boundary/explicit-deny wording).
Actual: 
- iam-explicit-deny-overrides-allow-s3-negative -> AssertionError: explicit Deny on s3:GetObject must refuse with AccessDenied; got {'ok': False, 'error': 'UnknownServiceError', 'message': "Unknown service: 's3'. Valid service names are: sqs, events, eventbridge, dynamodb"}
- lambda-role-permission-boundary-restricts-s3-negative -> AssertionError: a boundary that excludes s3 must refuse s3:PutObject with AccessDenied ...; got {'ok': False, 'error': 'UnknownServiceError', ...}
- lambda-role-s3-resource-arn-mismatch-negative -> AssertionError: expected AccessDenied for a role policy scoped to a different bucket ARN, got 'UnknownServiceError'
- cfn-update-iam-role-policy-reflected-in-lambda-execution-positive -> AssertionError: expected a 403/AccessDenied refusal before the policy exists, got 'UnknownServiceError'
- iam-role-policy-update-reflected-lambda-s3-access-positive -> same family (in-function S3 call never made)

Cause 2 — actual: iam-role-trust-policy-allows-lambda-assume-positive -> Error: Check timed out after 60.0s; lambda-role-wildcard-resource-s3-access-positive -> Error: Check timed out after 60.0s.

## Steps to Reproduce
Cause 1: create a bucket and a Lambda whose handler calls boto3.client('s3').put_object(...). Attach (a) an s3:* allow, (b) an allow plus an explicit Deny on s3:GetObject, (c) a permissions boundary excluding s3, (d) a policy scoped to a different bucket ARN. Invoke in each case. Expected: success for (a), HTTP 403 AccessDenied with the policy-type clause for (b)-(d). Observed: UnknownServiceError for all.
Cause 2: create a role trusting lambda.amazonaws.com with an s3:* wildcard policy and a function using it; invoke and poll for the written object — no verdict appears within 60 s.
