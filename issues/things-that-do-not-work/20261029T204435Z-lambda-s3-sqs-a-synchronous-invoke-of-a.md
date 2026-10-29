Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__sqs.cli-invoke-lambda-manual-test-event-positive
Issue-Key: lambda__s3__sqs.cli-invoke-lambda-manual-test-event-positive|_run_shell#10|AssertionError|raise AssertionError(
Issue-Commit: 78295efc92da5c3ab6dc723b39af1e4944870efd

# lambda__s3__sqs: a synchronous Invoke of a deployed function answers 501 NotImplemented, so `aws lambda invoke` with a synthetic S3 event never runs the handler and writes no object

> _Note: this issue was identified against the code base at commit `78295efc92da5c3ab6dc723b39af1e4944870efd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: a RequestResponse (default) Invoke of a function that was created successfully (zip code, python3.12, Active state) is refused by the emulator instead of running the handler and returning its result, so the CLI-driven manual-test-event path never produces the function's side effect (an S3 PutObject) or its return payload.

AWS: Smithy (services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model) models Invoke as POST /2015-03-31/functions/{FunctionName}/invocations with http code 200. The output shape InvocationResponse carries StatusCode (httpResponseCode), Payload (httpPayload — the handler's return value), ExecutedVersion (X-Amz-Executed-Version) and FunctionError (X-Amz-Function-Error, present ONLY when the function itself raised). For a successful synchronous invocation AWS answers HTTP 200, no X-Amz-Function-Error header, and the JSON the handler returned as the body; the invocation's own side effects (here s3:PutObject of processed/<key> into the OUT_BUCKET named by the function's environment) are durably visible afterwards. Every refusal Invoke is allowed to make is modeled and client- or server-classified (InvalidRequestContentException 400, RequestTooLargeException 413, ResourceNotFoundException 404, ResourceNotReadyException 502, TooManyRequestsException 429, ServiceException 500, …) — 501 NotImplemented is not among them, and the model documents no case in which a valid synchronous invocation of an Active function is refused.

Emulator: services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs states it outright — "Running the deployment package is NOT implemented: this emulator has no in-process runtime for a Lambda zip archive or container image, so `RequestResponse` answers HTTP 501". In the replay, `aws lambda invoke --function-name <fn> --payload file://<event.json> --cli-binary-format raw-in-base64-out <out>` exits non-zero against the emulator, the check falls back to the CLI-v1-style form without --cli-binary-format, and that second attempt dies in the CLI's own argument handling (`aws: [ERROR]: Invalid base64: "{\"Records\": [...]}"`, rc=255) — so the reported stderr is the client-side consequence of the first, service-side refusal. No `processed/incoming/testfile.txt` marker object is ever written, and the subsequent GetObject assertion can never pass.

Why: any client that drives a function synchronously — the AWS CLI's `lambda invoke`, an SDK Invoke, a manual test event from the console path, or any cross-service workflow whose step is "invoke this function and read its answer" — gets a 501 where AWS returns 200 plus the handler's payload. 501 is not a modeled Invoke error, so SDKs surface it as an unmodeled/unknown server failure rather than any retryable or actionable AWS error code, and every downstream effect the handler would have performed (here the S3 write that the check then reads back) simply does not exist. Nothing distinguishes "the function ran and failed" (HTTP 200 + X-Amz-Function-Error) from "the service would not run it".

Fix: the Invoke operation handler for InvocationType=RequestResponse (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs) must execute the resolved function's deployment package and answer HTTP 200 with the handler's return value as Payload, ExecutedVersion set to the resolved version, and X-Amz-Function-Error only when the handler itself raised; the function runtime that executes a zip package (and the state that records its side effects through the other services' APIs) is the component that must exist behind it. Refusals must stay within the modeled error set.

Related: the open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md describes the same 501 on Invoke, and 20261024T055421Z-lambda-opensearch-s3-lambda-invoke-cannot-execute-a-deployment-package-so.md the missing package execution; this check was NOT marked as owned by either, so it is filed here and those issues are named rather than duplicated — if they are fixed, re-replay the check below to confirm this path is covered.

Traces: lambda__s3__sqs.cli-invoke-lambda-manual-test-event-positive.

## Evidence
Check: lambda__s3__sqs.cli-invoke-lambda-manual-test-event-positive [mismatch].

Expected (AWS): `aws lambda invoke --function-name <fn> --payload file://<synthetic-s3-event.json> out.json` exits 0, prints invocation metadata with StatusCode 200 and NO FunctionError, writes the handler's return value ({"processed": ["processed/incoming/testfile.txt"]}) into out.json, and the handler's own s3:PutObject has created processed/incoming/testfile.txt in the OUT_BUCKET whose body is {"source_bucket": "<bucket>", "source_key": "incoming/testfile.txt"}.

Actual (emulator): the first invoke (`--cli-binary-format raw-in-base64-out`) exits non-zero — invoke.rs documents RequestResponse answering HTTP 501 NotImplemented, which is not a modeled Invoke error — and the check's CLI-v1 fallback then fails client-side:
  AssertionError: Command failed (rc=255):
  cmd: ['aws', 'lambda', 'invoke', '--function-name', 'sim2real-inv-fn-<id>', '--payload', 'file:///tmp/.../payload.json', '/tmp/.../out.json']
  stderr: aws: [ERROR]: Invalid base64: "{\"Records\": [{\"eventVersion\": \"2.1\", \"eventSource\": \"aws:s3\", \"eventName\": \"ObjectCreated:Put\", \"s3\": {\"bucket\": {\"name\": \"sim2real-inv-bkt-<id>\", ...}, \"object\": {\"key\": \"incoming/testfile.txt\"}}}]}"
No marker object is written, so the follow-up GetObject of processed/incoming/testfile.txt has nothing to read. Function names, bucket names and temp paths above are per-run values, not part of the requirement; the requirement is HTTP 200 + the handler's payload + the handler's side effect.

Source: services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs module doc — "Running the deployment package is NOT implemented … so `RequestResponse` answers HTTP 501 rather than a payload no function produced".

## Steps to Reproduce
1. CreateBucket <out-bucket>; create an IAM role trusting lambda.amazonaws.com with an inline s3:PutObject grant on arn:aws:s3:::<out-bucket>/*.
2. CreateFunction with runtime python3.12, handler index.handler, a zip whose index.py reads event["Records"][*]["s3"] and calls s3.put_object(Bucket=os.environ["OUT_BUCKET"], Key="processed/"+key, Body=...); wait until GetFunction reports State=Active.
3. Invoke the function synchronously (InvocationType=RequestResponse, i.e. `aws lambda invoke --function-name <fn> --payload file://event.json out.json` with a synthetic S3 ObjectCreated:Put event as the payload).
4. AWS: exit 0, StatusCode 200, no FunctionError, out.json holds the handler's return value. Emulator: the invocation is refused with HTTP 501 NotImplemented.
5. GetObject(Bucket=<out-bucket>, Key="processed/incoming/testfile.txt") — AWS returns the marker object written by the handler; the emulator has no such object because the handler never ran.
