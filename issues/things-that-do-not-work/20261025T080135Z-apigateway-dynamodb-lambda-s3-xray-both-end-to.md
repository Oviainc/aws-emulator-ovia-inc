Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__s3__xray.apigw-lambda-ddb-s3-trace-tree-shape apigateway__dynamodb__lambda__s3__xray.apigw-trace-header-continuation-and-sampled-zero-rejects-recording
Issue-Key: apigateway__dynamodb__lambda__s3__xray.apigw-lambda-ddb-s3-trace-tree-shape|_get_stage_json#10|Timeout|time.sleep(3)
Issue-Key: apigateway__dynamodb__lambda__s3__xray.apigw-trace-header-continuation-and-sampled-zero-rejects-recording|_get_stage_json#12|Timeout|time.sleep(3)
Issue-Commit: 03f6053936a7494b34f494829c7d405d52c6969b

# apigateway__dynamodb__lambda__s3__xray: both end-to-end X-Ray tracing checks never complete — a traced REST API stage produces no trace, so the distributed-trace pipeline is unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `03f6053936a7494b34f494829c7d405d52c6969b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetTraces

## Description
Cause 1 of 1: nothing in the producer-to-X-Ray path works end to end, so both distributed-tracing checks run out the 60 s harness budget instead of producing an answer. Failing checks: apigateway__dynamodb__lambda__s3__xray.apigw-lambda-ddb-s3-trace-tree-shape, apigateway__dynamodb__lambda__s3__xray.apigw-trace-header-continuation-and-sampled-zero-rejects-recording.

AWS: recorded behavior only (no Smithy model describes producer-side tracing; the query side is modeled: Smithy xray BatchGetTraces POST /Traces -> Traces[].Segments[].Document, GetTraceSummaries POST /TraceSummaries -> TraceSummaries[].Id, both 200). A GET at the invoke URL of a REST API stage with tracingEnabled=true returns the AWS_PROXY integration's response with HTTP 200, and the edge stamps _X_AMZN_TRACE_ID=Root=1-<8hex>-<24hex>;Parent=<16hex>;Sampled=1 on the invocation. The resulting single trace, read back through BatchGetTraces, is one closed tree of documents from three independent producers — origin AWS::ApiGateway::Stage (the only parentless document, named <api>/<stage>, carrying a nested namespace="aws" subsegment named Lambda whose aws.function_name is the integration target), AWS::Lambda, and AWS::Lambda::Function (parent_id = the AWS::Lambda segment id) — plus the two downstream nodes X-Ray SYNTHESISES itself, AWS::DynamoDB::Table and AWS::S3::Bucket, each marked inferred:true and hanging from the handler's own aws-namespace subsegment. Subsegments the handler sends to the daemon over UDP at AWS_XRAY_DAEMON_ADDRESS are accepted and stored inside the segment their parent_id names. An inbound X-Amzn-Trace-Id Root=<id> is continued byte-for-byte with a fresh 16-hex Parent=; a syntactically invalid Root= is ignored and replaced with a server-generated, epoch-prefixed id rather than being fatal; Sampled=0 still executes the request (the handler's DynamoDB write lands) while NOTHING is recorded for that id (BatchGetTraces returns no segments for it and it never appears in GetTraceSummaries over a window in which a Sampled=1 id of the same batch is already indexed).

Emulator: neither check ever reaches an assertion — both are killed by the harness with 'Check timed out after 60.0s'. Nothing in the sequence (create table, create bucket, create a Mode=Active function on a role granting xray:PutTraceSegments, deploy an AWS_PROXY REST API to a tracingEnabled stage, GET the stage, then poll BatchGetTraces/GetTraceSummaries for the trace) returns a usable result inside the budget: the stage GET is wrapped in a 90 s bounded retry that only loops on 403/429/5xx or on a request that does not answer at all, and the trace polls (240 s) only exit once the expected origins appear, so a stage that neither serves 200 nor refuses outright, and a trace store that never receives a document, both manifest as this hang. No AWS::ApiGateway::Stage / AWS::Lambda / AWS::Lambda::Function segment, no daemon-delivered subsegment, and no inferred AWS::DynamoDB::Table / AWS::S3::Bucket node is ever observed, and the Sampled=0 negative arm is never reached.

Why: a client cannot observe its own application's distributed traces at all. Every AWS-native tracing workflow built on an API-fronted Lambda — trace-tree shape, edge-to-service linkage, downstream resource inference, caller trace-context continuation, and the sampling decision (an unsampled request must still do its work and still record nothing) — is silently unavailable, and the failure presents as an indefinite hang rather than an error a client can handle. Because the hang consumes the whole budget, no narrower divergence behind it can be graded either.

Fix: the execute-api stage data plane (the emulator's front-door routing for the deployed REST API stage endpoint and its AWS_PROXY integration dispatch) must answer the stage GET, mint/propagate the X-Amzn-Trace-Id context into the invocation environment, and the X-Ray side must accept segments from the API Gateway edge and the Lambda service, accept the handler's UDP daemon datagrams, stitch documents from several producers into one trace by trace_id/parent_id, synthesise the inferred downstream resource nodes, and honour the Sampled flag on ingestion. Related open issue 20261024T043741Z-apigateway-dynamodb-lambda-s3-xray-both-x-ray-9c23a5.md carries these same two check ids but is keyed on the stage GET's AssertionError (last status 404 UnknownService); the current failure mode is the harness cap, so the replay did not report these checks as owned by it — the two should be reconciled. Open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md (Lambda Invoke answers NotImplemented) and 20261025T014422Z-apigateway-api-gateway-a-deployed-rest-api-stage-serves.md describe adjacent causes on the same path.

Traces: apigateway__dynamodb__lambda__s3__xray.apigw-lambda-ddb-s3-trace-tree-shape, apigateway__dynamodb__lambda__s3__xray.apigw-trace-header-continuation-and-sampled-zero-rejects-recording.

## Evidence
Replay output, both checks in this group:

[mismatch] apigateway__dynamodb__lambda__s3__xray.apigw-lambda-ddb-s3-trace-tree-shape
  Error: Check timed out after 60.0s

[mismatch] apigateway__dynamodb__lambda__s3__xray.apigw-trace-header-continuation-and-sampled-zero-rejects-recording
  Error: Check timed out after 60.0s

Expected (recorded against real AWS 2026-09-13, us-west-2): the stage GET answers 200 with the handler's JSON, whose raw_trace_header matches ^Root=1-[0-9a-f]{8}-[0-9a-f]{24};Parent=[0-9a-f]{16};Sampled=1$; BatchGetTraces([root]) then returns exactly one trace whose flattened documents cover origins {AWS::ApiGateway::Stage, AWS::Lambda, AWS::Lambda::Function, AWS::DynamoDB::Table, AWS::S3::Bucket}, form a single tree (one parentless document, n-1 parent edges, acyclic), carry the handler's two UDP-delivered aws-namespace subsegments nested under the AWS::Lambda::Function segment with the request_id DynamoDB/S3 actually returned, and mark the two downstream resource documents inferred:true. In the second check: Root=<caller id>;Sampled=1 is continued under the caller's id and indexed by GetTraceSummaries, while Root=<other id>;Sampled=0 writes its DynamoDB item yet yields no segment from BatchGetTraces and no entry in GetTraceSummaries over the same window.

Actual: no assertion is ever reached — the check is terminated at the 60 s harness cap, so no trace document, no stage segment and no sampling decision is observable. The checks' own internal waits (90 s bounded retry on the stage GET, 240 s polls on BatchGetTraces/GetTraceSummaries) exceed that cap, which is why a non-serving stage or an empty trace store surfaces as a timeout rather than a diagnosis.

## Steps to Reproduce
1. Create a PAY_PER_REQUEST DynamoDB table with an S hash key 'pk' and an S3 bucket.
2. Create a python3.12 Lambda with TracingConfig={'Mode':'Active'} on a role granting xray:PutTraceSegments, xray:PutTelemetryRecords, dynamodb:PutItem on the table and s3:PutObject on the bucket; the handler echoes os.environ['_X_AMZN_TRACE_ID'], does a PutItem and a PutObject, and sends one aws-namespace subsegment per call over UDP to AWS_XRAY_DAEMON_ADDRESS with trace_id=Root and parent_id=Parent.
3. Create a REST API with a GET /x AWS_PROXY integration to that function, add the apigateway.amazonaws.com lambda permission, create_deployment to stage 'traced' and update_stage /tracingEnabled=true (and a second stage with tracingEnabled left false for the control arm).
4. GET the stage invoke URL. Expected: 200 with the handler JSON and a well-formed Root=...;Parent=...;Sampled=1 header. Then poll xray.batch_get_traces(TraceIds=[Root]) for the five-origin tree described above.
5. Repeat with request header X-Amzn-Trace-Id: Root=<minted id>;Sampled=1, then with Sampled=0, then with no header, then with Root=not-a-trace-id, and compare BatchGetTraces / GetTraceSummaries for the sampled and unsampled ids over one window.
6. Observe that neither sequence ever yields a trace: both checks run to the 60 s harness cap.
7. Or replay the two check ids in trace_ids.
