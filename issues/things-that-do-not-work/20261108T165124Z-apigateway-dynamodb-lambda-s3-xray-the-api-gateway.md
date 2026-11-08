Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__s3__xray.apigw-lambda-ddb-s3-trace-tree-shape
Issue-Key: apigateway__dynamodb__lambda__s3__xray.apigw-lambda-ddb-s3-trace-tree-shape|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Commit: 0a1e14ce9c5d04d4281ea317c037d5ece999a2b8

# apigateway__dynamodb__lambda__s3__xray: the API Gateway → Lambda → DynamoDB/S3 → X-Ray trace-tree flow never completes, so the check exhausts its 180s budget (1 cause)

> _Note: this issue was identified against the code base at commit `0a1e14ce9c5d04d4281ea317c037d5ece999a2b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTraceSummaries

## Description
Cause 1 of 1 — the end-to-end distributed-tracing flow over API Gateway, Lambda, DynamoDB, S3 and X-Ray never reaches a terminal result; the check is cut off by its 180s budget rather than producing a verdict.
AWS: a REST API with one AWS_PROXY GET resource integrated to a Lambda function, deployed to a stage with tracingEnabled=true, answers the stage invoke URL with 200 within seconds; the handler reads the _X_AMZN_TRACE_ID it was handed, performs its DynamoDB and S3 calls, and emits segment documents to AWS_XRAY_DAEMON_ADDRESS over UDP; X-Ray then serves one AWS::ApiGateway::Stage segment, one AWS::Lambda / AWS::Lambda::Function pair and the DynamoDB and S3 subsegments, each hanging off the parent id the trace header carried, through GetTraceSummaries/BatchGetTraces. The whole flow is observable well inside the check's budget (its own stage poll is bounded at 90s and demands a 200 once it runs out).
Emulator: no terminal result at all — the replay reports only 'Check timed out after 180.0s', so some step in the chain never converges within the budget.
Hypothesis: the two bounded waits in this flow are the stage invoke (retried only on 403/429/5xx for up to 90s, then required to be 200) and the subsequent X-Ray segment poll; the timeout is consistent with the deployed stage never serving 200 and/or no segment documents ever becoming retrievable, but neither the model nor the replay output states which, so the first fix step is to determine where the flow stalls.
Why: a client instrumenting a serverless request path cannot observe any trace tree, and an automated check of that path hangs for its whole budget instead of failing fast — turning one missing capability into a wall-clock cost on every run.
Fix: whichever component stalls in this chain — the execute-api stage invoke path for an AWS_PROXY integration with tracingEnabled, and the X-Ray segment ingestion/retrieval that must make the emitted documents queryable. Several open issues describe the execute-api invoke path failing in other service groupings (e.g. 20261107T185637Z-apigateway-cloudfront-s3-4-net-new-causes-the-execute-api-invoke.md); none of them carries this check.
Traces: apigateway__dynamodb__lambda__s3__xray.apigw-lambda-ddb-s3-trace-tree-shape.

## Evidence
Expected: the check deploys the API, invokes the stage URL (200 within its bounded 90s retry), and then asserts the shape of the X-Ray trace tree (exactly one document per origin, each child's parent_id resolving to its container segment).
Actual: no response and no assertion is ever reached. Replay detail: `Error: Check timed out after 180.0s`.

## Steps to Reproduce
1. Create a DynamoDB table (one S hash key 'pk', PAY_PER_REQUEST) and wait for ACTIVE; create a default bucket.
2. Create a Lambda function whose handler reads _X_AMZN_TRACE_ID, calls DynamoDB and S3, and sends segment documents to AWS_XRAY_DAEMON_ADDRESS over UDP.
3. Create a REST API with one resource, PUT METHOD GET authorizationType=NONE, PUT INTEGRATION type=AWS_PROXY to the function, lambda:AddPermission for apigateway.amazonaws.com, CreateDeployment to a stage and UpdateStage with /tracingEnabled=true.
4. GET the stage invoke URL and expect 200 with the handler's JSON; then poll X-Ray GetTraceSummaries/BatchGetTraces for the emitted trace.
5. Observe that the flow does not complete within 180s.
6. Replay: apigateway__dynamodb__lambda__s3__xray.apigw-lambda-ddb-s3-trace-tree-shape.
