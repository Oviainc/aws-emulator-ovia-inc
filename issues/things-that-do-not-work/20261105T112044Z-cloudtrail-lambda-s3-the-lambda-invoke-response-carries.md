Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__lambda__s3.obs-lambda-3-data-events-s3
Issue-Key: cloudtrail__lambda__s3.obs-lambda-3-data-events-s3|_invoke#37|KeyError|return resp["ResponseMetadata"]["RequestId"]
Issue-Commit: c1082b91874a267e16e5e882b92de2a31503a88d

# cloudtrail__lambda__s3: the Lambda Invoke response carries no request-id header, so ResponseMetadata.RequestId is absent and an Invoke data event cannot be correlated

> _Note: this issue was identified against the code base at commit `c1082b91874a267e16e5e882b92de2a31503a88d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the emulator's Lambda Invoke response omits the AWS request-id response header, so an SDK client finds no `RequestId` in `ResponseMetadata` and cannot correlate the invocation with the CloudTrail Invoke data event it produced. Failing check: cloudtrail__lambda__s3.obs-lambda-3-data-events-s3.

AWS: every Lambda data-plane response carries the invocation's request id in a response header (`x-amzn-RequestId`, with `x-amz-request-id` on the S3-style surfaces), which is what the AWS SDKs surface as `ResponseMetadata.RequestId`; the same id appears as the CloudTrail record's `requestID`, which is how a caller joins an Invoke to its audit record. Smithy (lambda, Invoke): HTTP POST /2015-03-31/functions/{FunctionName}/invocations, success code 200, with StatusCode bound to httpResponseCode, FunctionError to `X-Amz-Function-Error`, LogResult to `X-Amz-Log-Result`, ExecutedVersion to `X-Amz-Executed-Version` and Payload to the HTTP payload — the request id is not a modelled output member but a protocol-level response header every AWS service emits.

Emulator: the Invoke answers without that header, so botocore's `ResponseMetadata` has no `RequestId` key at all and reading it raises `KeyError: 'RequestId'`. This is the absence of the header, not a difference in its value — the value is per-invocation and is never itself a requirement.

Why: a client cannot join a Lambda invocation to the CloudTrail Invoke data event it generated (requestID), nor quote a request id to support when an invocation misbehaves; any tooling that logs or de-duplicates on ResponseMetadata.RequestId breaks outright.

Fix: the Lambda Invoke handler's response serializer must emit the per-invocation request id as a response header on every Invoke response, success and failure alike (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs, or the shared response-metadata layer that frames every reply).

The open cloudtrail issues for this estate (20261024T114720Z-cloudtrail-event-history-is-empty-for-everything-but.md, 20261102T102343Z-cloudtrail-s3-2-net-new-root-causes-s3.md) describe CloudTrail's own gaps under other groupings and do not carry this check's replay key.

Traces: see trace_ids.

## Evidence
Expected (AWS): `lambda.invoke(...)` -> response whose `ResponseMetadata` contains `RequestId` (populated from the `x-amzn-RequestId` response header), and the same id appears as `requestID` on the CloudTrail Invoke data event.

Actual (emulator): `KeyError: 'RequestId'` at check_cloudtrail__lambda__s3_obs_lambda_3_data_events_s3.py line 224, `return resp["ResponseMetadata"]["RequestId"]` — the key is missing because the response carries no request-id header. The check never reaches its CloudTrail assertions.

## Steps to Reproduce
1. CreateFunction (any runtime) and Invoke it synchronously with an SDK client.
2. Inspect the raw response headers and the SDK's `ResponseMetadata`: no `x-amzn-RequestId` header is present and `ResponseMetadata` has no `RequestId` key.
3. Re-replay cloudtrail__lambda__s3.obs-lambda-3-data-events-s3 to confirm.
