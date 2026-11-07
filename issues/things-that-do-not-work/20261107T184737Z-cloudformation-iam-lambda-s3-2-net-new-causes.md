Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__s3.iam-role-trust-policy-allows-lambda-assume-positive cloudformation__iam__lambda__s3.lambda-role-wildcard-resource-s3-access-positive cloudformation__iam__lambda__s3.cfn-custom-resource-role-least-privilege-positive
Issue-Key: cloudformation__iam__lambda__s3.iam-role-trust-policy-allows-lambda-assume-positive|run#10|AssertionError|assert result["ok"] is True, result
Issue-Key: cloudformation__iam__lambda__s3.lambda-role-wildcard-resource-s3-access-positive|run#9|AssertionError|assert put_a["ok"] is True, put_a
Issue-Key: cloudformation__iam__lambda__s3.cfn-custom-resource-role-least-privilege-positive|_poll_until#10|Timeout|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 8662809dae52673eae9c053e761f1b93b4d6fc67

# cloudformation__iam__lambda__s3: 2 net-new causes — the in-function AWS SDK has no S3 client (UnknownServiceError), and a custom-resource-driven create+update stack never settles

> _Note: this issue was identified against the code base at commit `8662809dae52673eae9c053e761f1b93b4d6fc67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (in-function boto3.client("s3")) / CreateStack+UpdateStack with a Lambda-backed Custom::Resource

## Description
This issue carries 2 net-new root causes.

Cause 1 of 2: a handler running in the emulator's Lambda runtime cannot obtain an S3 client — constructing one raises UnknownServiceError — so no in-function S3 call happens and every permission-boundary conclusion drawn from one is vacuous.
Failing check ids for this cause: cloudformation__iam__lambda__s3.iam-role-trust-policy-allows-lambda-assume-positive, cloudformation__iam__lambda__s3.lambda-role-wildcard-resource-s3-access-positive.
AWS: recorded behavior only — the Lambda execution environment's bundled SDK is not described by a Smithy model. The recording shows that a python handler in a Lambda whose execution role trusts lambda.amazonaws.com and carries an s3 grant can create an S3 client and perform the call: with an inline policy allowing s3:GetObject the handler reads the object and returns {"ok": true, ...} with its content, and with a wildcard policy (Action s3:*, Resource *) the handler both writes and reads objects in two independently created buckets, proving the broad grant is honoured generically rather than for one special-cased bucket ARN.
Emulator: the handler returns {'ok': False, 'error': 'UnknownServiceError'} — the error raised when the bundled SDK has no service model for the requested client, i.e. the S3 client cannot be constructed at all. The error is not an authorization outcome and is not an S3 error code, so neither check ever reaches the S3 call whose authorization it exists to verify.
Why: no Lambda function can use S3. Every S3-dependent workload (object processing, uploads, config fetches) is unrunnable, and — because the failure is a client-construction error rather than AccessDenied or a successful read — a caller cannot distinguish "the role does not permit this" from "the runtime cannot talk to S3", which makes the entire IAM-on-Lambda story untestable.
Fix: the in-process Lambda runtime's bundled AWS SDK — register the s3 service client (alongside the services it already carries) and route its calls to the emulator's S3 endpoint under the function's execution-role credentials, so an in-function S3 call either succeeds or fails with S3's own error.

Cause 2 of 2: a stack whose Custom::Resource is backed by a Lambda provider never settles across a CREATE followed by an UPDATE — the check is killed at its 180 s budget with no stack status or provider effect observed.
Failing check ids for this cause: cloudformation__iam__lambda__s3.cfn-custom-resource-role-least-privilege-positive.
AWS: recorded behavior only — the custom-resource provider protocol is not in the Smithy model. The recording shows a Custom::Resource whose provider Lambda's role is scoped to exactly s3:PutObject on one bucket writing the v1 object during stack CREATE, the stack reaching CREATE_COMPLETE, an UpdateStack then driving the provider's Update event which writes the v2 object, the stack reaching UPDATE_COMPLETE, and the v1 object left unchanged — i.e. the narrowly scoped policy suffices for both lifecycle events.
Emulator: the suite reports exactly 'Error: Check timed out after 180.0s'. No stack status, no provider invocation and neither object is observed, so the least-privilege conclusion cannot be drawn at all.
Why: a Lambda-backed custom resource — the standard extension point for provisioning anything CloudFormation does not model — hangs the stack. A client polling for CREATE_COMPLETE or UPDATE_COMPLETE blocks indefinitely, and the side effects the provider exists to perform never land.
Fix: the CloudFormation custom-resource provisioner's Lambda ServiceToken path — invoke the provider for each lifecycle event, honour its SUCCESS/FAILED response put to ResponseURL, and drive the stack to a terminal status within the operation's bounds instead of waiting indefinitely.

Traces: the three check ids in trace_ids, grouped per cause above.

## Evidence
Cause 1 — Expected: the handler builds an S3 client under its execution role and returns {"ok": true, ...} (object content for the GetObject case; successful writes and reads in both buckets for the wildcard case). Actual: AssertionError: {'ok': False, 'error': 'UnknownServiceError'} — for both iam-role-trust-policy-allows-lambda-assume-positive (assert result["ok"] is True) and lambda-role-wildcard-resource-s3-access-positive (assert put_a["ok"] is True).

Cause 2 — Expected: CREATE writes the v1 object and the stack reaches CREATE_COMPLETE; UpdateStack writes the v2 object and the stack reaches UPDATE_COMPLETE with v1 unchanged. Actual: Error: Check timed out after 180.0s — no stack status and neither object observed.

## Steps to Reproduce
Cause 1: create a bucket and put an object; create a role trusting lambda.amazonaws.com with an inline policy allowing s3:GetObject on that object (and separately one allowing s3:* on *); CreateFunction with that role and a python handler that does boto3.client("s3") and get_object/put_object, returning {"ok": True} on success; Invoke it and read the payload — it reports UnknownServiceError.
Cause 2: create a bucket; create a provider Lambda role scoped to s3:PutObject on that bucket only; create the provider function; CreateStack with a Custom:: resource whose ServiceToken is that function and whose properties name key v1; wait for CREATE_COMPLETE; UpdateStack naming key v2; wait for UPDATE_COMPLETE — neither status is ever reached.
Or replay the three check ids in trace_ids.
