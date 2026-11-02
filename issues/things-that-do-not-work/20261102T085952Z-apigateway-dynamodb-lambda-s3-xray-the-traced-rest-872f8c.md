Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__s3__xray.apigw-lambda-ddb-s3-trace-tree-shape
Issue-Key: apigateway__dynamodb__lambda__s3__xray.apigw-lambda-ddb-s3-trace-tree-shape|_http_get#10|Timeout|return exc.code, exc.read().decode()
Issue-Commit: fec48c0e4a4ef614c94c9aeb73915e609cba5726

# apigateway__dynamodb__lambda__s3__xray: the traced REST stage -> Lambda -> DynamoDB/S3 trace tree is still unobservable — the stage GET never answers, so the check runs out the harness cap

> _Note: this issue was identified against the code base at commit `fec48c0e4a4ef614c94c9aeb73915e609cba5726`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteApiInvoke

## Description
Cause 1 of 1: a GET to a freshly deployed REST API stage whose GET method has an AWS_PROXY integration onto a Mode=Active Lambda never produces an HTTP response at all, so every X-Ray assertion the check builds on that response is unreachable and the check is cut off by the harness at its 60 s cap. The check's own helper retries only on 403/429/5xx for 90 s and never observed any response, so the hang — not the harness budget — is what consumes the run: a stage that answered promptly would let the check complete well inside the cap.

RELATED OPEN ISSUES that look similar but do NOT carry this check's replay key (the replay report marks none of them as owning it), so this is filed separately rather than as a duplicate: 20261024T043741Z-apigateway-dynamodb-lambda-s3-xray-both-x-ray-9c23a5.md, 20261025T080135Z-apigateway-dynamodb-lambda-s3-xray-both-end-to.md (the report marks it as owning only the sibling continuation check), 20261027T085253Z-apigateway-dynamodb-lambda-s3-xray-caller-trace-context.md, 20261027T205529Z-apigateway-dynamodb-lambda-s3-xray-the-traced-rest.md, 20261030T003023Z-apigateway-dynamodb-lambda-s3-xray-x-ray-trace-header-continuation-through-a-rest.md and 20261101T204543Z-apigateway-dynamodb-lambda-s3-xray-both-distributed-tracing.md. The cause described here is the same stage-invoke hang those name; it is still reproducing at this commit.

AWS: recorded behavior only — the execute-api stage invoke is not a modeled Smithy operation; AWS documents the tracing behaviour at https://docs.aws.amazon.com/xray/latest/devguide/xray-services-apigateway.html. A GET to `<rest-api-id>.execute-api.<region>.amazonaws.com/<stage>/<path>` on a deployed stage with an AWS_PROXY integration is answered HTTP 200 carrying the function's proxy response (the `body` string and `headers` the handler returned) within the stage's integration timeout. With the stage's `tracingEnabled=true` the edge stamps the invocation's `_X_AMZN_TRACE_ID` as `Root=1-<8 hex>-<24 hex>;Parent=<16 hex>;Sampled=1`, and the resulting trace is ONE tree: exactly one root document with `origin` `AWS::ApiGateway::Stage` and `name` `<api>/<stage>`, holding an aws-namespace `Lambda` subsegment that parents the `AWS::Lambda` segment, which parents the `AWS::Lambda::Function` segment; the handler's own aws-namespace subsegments nest under the function segment with `aws.operation` PutItem/PutObject and the `aws.request_id` the data-plane call actually returned, and X-Ray synthesises `inferred: true` `AWS::DynamoDB::Table` / `AWS::S3::Bucket` nodes hanging off them. On a stage with `tracingEnabled=false` no `AWS::ApiGateway::Stage` segment exists and the `AWS::Lambda` segment is the root. The requirements are those shapes and the forms `^1-[0-9a-f]{8}-[0-9a-f]{24}$` (Root) and `^[0-9a-f]{16}$` (span id); the api id, function/table/bucket names, span ids and timestamps of any one run are per-run values and not part of the contract.

Emulator: the stage GET never returns — no status, no body, no headers, no `_X_AMZN_TRACE_ID` echo. Verdict `mismatch`, detail `Error: Check timed out after 60.0s`. Because no root trace id is ever obtained, BatchGetTraces is never reached for a real trace and none of the tree assertions (single stage-rooted document, the Lambda two-segment shape, the handler's nested aws subsegments and their request ids, the inferred DynamoDB/S3 nodes, parent/child duration containment, the untraced-stage arm) produces a verdict.

Why: a client fronting a Lambda with API Gateway gets no response at all instead of its proxy response — not an error it can handle but a stalled socket, which drives SDK timeout/retry logic rather than status-based handling. Every instrumentation guarantee layered on that path is unverifiable: trace context reaching the invocation environment, the edge minting a Parent span, the stage being the trace root, and the inferred downstream resource nodes for the DynamoDB and S3 calls the handler really made.

Fix: the apigateway execute-api stage-invoke data plane — the deployed-stage request router and its AWS_PROXY integration invoker must serve the request and return the function's proxy response, and the trace-context propagation that handler owns must set the invocation's `_X_AMZN_TRACE_ID` and emit the `AWS::ApiGateway::Stage` segment when the stage has `tracingEnabled=true`.

Traces: apigateway__dynamodb__lambda__s3__xray.apigw-lambda-ddb-s3-trace-tree-shape.

## Evidence
Expected (real AWS):
- `GET <stage-url>/x?ddb_sub_id=<16hex>&s3_sub_id=<16hex>&k=traced-probe` on the tracing-enabled stage answers HTTP 200 with the handler's JSON body, whose `raw_trace_header` matches `^Root=1-[0-9a-f]{8}-[0-9a-f]{24};Parent=[0-9a-f]{16};Sampled=1(;.*)?$` and whose `ddb_request_id`/`s3_request_id` are the request ids PutItem and PutObject returned.
- `batch_get_traces(TraceIds=[<root>])` then yields one tree: exactly one root document with `origin == 'AWS::ApiGateway::Stage'` and `name` ending `/traced`; its aws-namespace `Lambda` subsegment parents the `AWS::Lambda` segment; `AWS::Lambda::Function.parent_id == AWS::Lambda.id`; the handler's two UDP-reported subsegments nest under the function segment with `namespace: 'aws'`, `aws.operation` PutItem/PutObject, `aws.table_name`/`aws.bucket_name` set and `aws.request_id` equal to the ids echoed in the body; `AWS::DynamoDB::Table` and `AWS::S3::Bucket` documents carry `inferred: true` and hang from those subsegments; the tree has edges == n-1 with no dangling parent and no cycle.
- On the `untraced` stage the same GET answers 200 and no `AWS::ApiGateway::Stage` segment is produced.

Actual (emulator): verdict `mismatch`, detail `Error: Check timed out after 60.0s`. The GET inside the check's `_get_stage_json`/`_http_get` helper never returns any HTTP response; the helper's own 90 s retry loop (403/429/5xx only) observed nothing to retry on. No trace header, no trace id, and therefore no BatchGetTraces verdict at all. The DynamoDB table, the S3 bucket, the IAM role, the Mode=Active python3.12 function and the two deployments (`traced` with tracingEnabled=true, `untraced`) were all created before the GET, so the failure is located at the stage invoke.

## Steps to Reproduce
1. Create an on-demand DynamoDB table with a single `S` hash key `pk` and wait for ACTIVE; create a default-settings S3 bucket.
2. Create an IAM role trusted by `lambda.amazonaws.com` with an inline policy granting `xray:PutTraceSegments`, `xray:PutTelemetryRecords`, `logs:*`, `dynamodb:PutItem` on the table and `s3:PutObject` on the bucket.
3. Create a `python3.12` Lambda on that role with `TracingConfig={'Mode':'Active'}` and env `PROBE_TABLE`/`PROBE_BUCKET`; its handler echoes `os.environ['_X_AMZN_TRACE_ID']` with the parsed Root/Parent/Sampled, performs a PutItem and a PutObject, and sends two aws-namespace subsegment documents to `AWS_XRAY_DAEMON_ADDRESS` over UDP. Wait for `State == 'Active'`.
4. Create a REST API with resource `/x`, a `GET` method (`authorizationType=NONE`) and an `AWS_PROXY` integration (`integrationHttpMethod=POST`) onto that function, plus `lambda:AddPermission` for `apigateway.amazonaws.com`.
5. `create_deployment` to stage `traced`, then `update_stage` with `[{'op':'replace','path':'/tracingEnabled','value':'true'}]` and confirm `get_stage` reports `tracingEnabled: true`; also deploy stage `untraced` at defaults.
6. `GET <traced-stage-url>/x?ddb_sub_id=<16hex>&s3_sub_id=<16hex>&k=traced-probe` — observe the request never returns instead of answering 200 with the handler's proxy body.
7. Consequently `batch_get_traces(TraceIds=[<root from the body>])` can never be exercised, so the stage-rooted trace tree, the Lambda two-segment shape and the inferred DynamoDB/S3 nodes are unobservable; the same GET against the `untraced` stage also never answers.
8. Re-replay apigateway__dynamodb__lambda__s3__xray.apigw-lambda-ddb-s3-trace-tree-shape to confirm the fix.
