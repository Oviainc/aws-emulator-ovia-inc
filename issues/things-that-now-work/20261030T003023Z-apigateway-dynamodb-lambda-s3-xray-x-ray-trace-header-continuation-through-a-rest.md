Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__s3__xray.apigw-trace-header-continuation-and-sampled-zero-rejects-recording
Issue-Key: apigateway__dynamodb__lambda__s3__xray.apigw-trace-header-continuation-and-sampled-zero-rejects-recording|_http_get#12|Timeout|return exc.code, exc.read().decode()
Issue-Commit: c1b8e21acf710dd592fea1840f63adefca5c3a99

# X-Ray trace-header continuation through a REST stage never answers, so Root=/Sampled= handling is unobservable

> _Note: this issue was identified against the code base at commit `c1b8e21acf710dd592fea1840f63adefca5c3a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteApiInvoke

## Description
Cause 1 of 1: GET requests to a deployed REST stage carrying a caller-supplied X-Amzn-Trace-Id never produce a response, so neither trace continuation nor the Sampled=0 suppression path can be observed.

AWS: recorded behavior only for the data plane (the execute-api stage invoke is not a modeled Smithy operation); AWS documents the behaviour at https://docs.aws.amazon.com/xray/latest/devguide/xray-services-apigateway.html. A caller-supplied `X-Amzn-Trace-Id: Root=1-<8 hex>-<24 hex>;Sampled=1` is CONTINUED: the integration sees _X_AMZN_TRACE_ID with the same Root=, a Parent= that the edge minted as a fresh 16-hex span id under that root (none was supplied), and Sampled=1 — and the request is served 200 with the handler's JSON body. On an otherwise identical request with `Sampled=0` the work still happens (the DynamoDB item and the S3 object are written and the response is 200) but nothing is recorded, so GetTraceSummaries never reports that root. A syntactically invalid `Root=not-a-trace-id` does not fail the request either. The structural requirements are the regexes themselves — Root matches ^1-[0-9a-f]{8}-[0-9a-f]{24}$ and the minted Parent matches ^[0-9a-f]{16}$ — the concrete ids in any one run are per-run values, not part of the contract.

Emulator: none of the four stage GETs (Sampled=1, Sampled=0, no header, malformed Root) ever returns. Verdict `mismatch`, detail `Error: Check timed out after 60.0s`; no status, no body, and no trace-header echo was ever observed, and the X-Ray assertions are never reached.

Why: a client instrumenting an API-Gateway-fronted function cannot verify that its own trace context is propagated, that the edge mints a parent span, or that Sampled=0 suppresses recording without suppressing the work. Request-level sampling decisions and cross-service trace joins silently cannot be exercised; the caller just hangs.

Fix: the apigateway REST stage-invoke data-plane handler (and the trace-header propagation it is responsible for when the stage/function has Active tracing) — it must serve the request and set the integration's _X_AMZN_TRACE_ID from the incoming header, minting Parent= when the caller supplied none.

Related open issue 20261025T080135Z-apigateway-dynamodb-lambda-s3-xray-both-end-to.md covers the end-to-end trace-tree shape check in this group; it does not carry this check's replay key, so this cause is filed separately.

Traces: apigateway__dynamodb__lambda__s3__xray.apigw-trace-header-continuation-and-sampled-zero-rejects-recording.

## Evidence
Expected (real AWS): GET <stage-url>/x?k=arm-a&sub_id=<id> with header X-Amzn-Trace-Id: Root=<minted-root>;Sampled=1 returns 200 and a body whose root equals the supplied Root, whose parent matches ^[0-9a-f]{16}$, and whose sampled is "1"; the same request with Sampled=0 returns 200, writes its DynamoDB item and S3 object, and that root never appears in GetTraceSummaries.

Actual (emulator): verdict `mismatch`, detail `Error: Check timed out after 60.0s`. No HTTP response was produced for any of the four stage GETs, so no trace header, status or body was ever observed.

## Steps to Reproduce
1. Create a DynamoDB table and an S3 bucket.
2. Create a python3.12 Lambda with TracingConfig Mode=Active on a role granting xray:PutTraceSegments/PutTelemetryRecords, dynamodb:PutItem and s3:PutObject, whose handler echoes os.environ['_X_AMZN_TRACE_ID'] and its parsed Root/Parent/Sampled.
3. Create a REST API with an AWS_PROXY method onto it and deploy to a stage with tracing enabled.
4. GET <stage-url>/x?k=arm-a&sub_id=<16-hex> with header X-Amzn-Trace-Id: Root=1-<8hex>-<24hex>;Sampled=1 — observe the request is never answered instead of returning 200 with the continued Root and a minted 16-hex Parent.
5. Repeat with Sampled=0, with no X-Amzn-Trace-Id at all, and with Root=not-a-trace-id;Sampled=1 — all four never answer.
