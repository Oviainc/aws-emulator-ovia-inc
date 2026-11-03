Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__iam__lambda.granted-caller-reads-the-seeded-agreement-verbatim-through-the-lambda-proxy apigateway__dynamodb__iam__lambda.agreement-rewritten-in-dynamodb-is-served-on-the-next-signed-get apigateway__dynamodb__iam__lambda.removing-the-invoke-permission-is-rejected-with-a-500-at-the-stage
Issue-Key: apigateway__dynamodb__iam__lambda.granted-caller-reads-the-seeded-agreement-verbatim-through-the-lambda-proxy|_invoke#5|Timeout|return exc.code, {k.lower(): v for k, v in (exc.headers or {}).items()}, exc.read().decode()
Issue-Key: apigateway__dynamodb__iam__lambda.agreement-rewritten-in-dynamodb-is-served-on-the-next-signed-get|_invoke#5|Timeout|return exc.code, {k.lower(): v for k, v in (exc.headers or {}).items()}, exc.read().decode()
Issue-Key: apigateway__dynamodb__iam__lambda.removing-the-invoke-permission-is-rejected-with-a-500-at-the-stage|_invoke#6|Timeout|return exc.code, {k.lower(): v for k, v in (exc.headers or {}).items()}, exc.read().decode()
Issue-Commit: a03f808379986133e0f00cdb9a2ead2b78e9a26e

# apigateway__dynamodb__iam__lambda: a deployed REST stage with an AWS_PROXY Lambda integration never produces any HTTP response, so the proxy data plane (200 body, live table reads, and the 500 after RemovePermission) is unobservable

> _Note: this issue was identified against the code base at commit `a03f808379986133e0f00cdb9a2ead2b78e9a26e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api deployed stage, AWS_PROXY/Lambda-proxy integration)

## Description
This issue carries 1 net-new root cause.

Cause 1 of 1: a request to a deployed REST API stage whose only method is AWS_IAM-authorized and wired to an AWS_PROXY (Lambda proxy) integration over a DynamoDB-backed handler never produces an HTTP response at all.
Failing check ids for this cause: apigateway__dynamodb__iam__lambda.agreement-rewritten-in-dynamodb-is-served-on-the-next-signed-get, apigateway__dynamodb__iam__lambda.granted-caller-reads-the-seeded-agreement-verbatim-through-the-lambda-proxy, apigateway__dynamodb__iam__lambda.removing-the-invoke-permission-is-rejected-with-a-500-at-the-stage.

AWS: recorded behavior only — the execute-api data plane is not described by the API Gateway control-plane Smithy model, so what is modelled here is only the control-plane setup (CreateRestApi/PutMethod authorizationType=AWS_IAM, PutIntegration type=AWS_PROXY, CreateDeployment/CreateStage, Lambda AddPermission/RemovePermission for principal apigateway.amazonaws.com). What the recordings show is that every request reaching a deployed stage is answered: (a) a SigV4-signed GET of a granted path is answered 200 with Content-Type: application/json and a body that is exactly what the proxy handler returned (the DynamoDB item serialized verbatim, nested maps and JSON-schema strings byte for byte), and an unseeded id gets the handler's own 404 body; (b) the stage serves live table state, so after an UpdateItem rewrites the item the next signed GET carries the new attribute values rather than a deployment-time snapshot; (c) once lambda RemovePermission drops the function's apigateway.amazonaws.com statement for that stage, the same signed GET is answered 500 with response header x-amzn-ErrorType: InternalServerErrorException and body {"message": "Internal server error"} — that body and that error type are fixed text with no variable parts (the path, api id, stage and item values come from the request and the emulator's state).
Emulator: no HTTP response is produced on the proxy path at any point. All three checks poll the stage invoke URL until the harness budget is exhausted and the suite reports exactly 'Error: Check timed out after 60.0s'. No status line, no headers and no body are ever observed — neither the 200 success, nor the live-read result, nor the 500 refusal after the invoke permission is removed.
Why: a client of a proxy-integrated REST API hangs until its own socket/SDK timeout instead of receiving any status. It cannot distinguish a successful backend response, a stale read, a backend failure and a missing lambda:InvokeFunction grant; retry and error-handling logic that keys on 200 vs 500/InternalServerErrorException never runs, and the proxy response contract (status, Content-Type, body passthrough) is entirely unverifiable.
Fix: the execute-api data-plane request path of the API Gateway service — stage/route resolution for a deployed stage, SigV4 verification at the stage, the AWS_PROXY integration invoke of the target Lambda function, the mapping of the handler's proxy response (statusCode/headers/body) back to the HTTP response, and the 500 InternalServerErrorException mapping when API Gateway itself is not permitted to invoke the backend. Every request that reaches a deployed stage must terminate in a response.
Related already-open issues describe this same surface but do not carry these three checks' replay keys, so the suite does not report them as owned and they must be closed alongside: 20261102T091623Z-apigateway-18-further-requests-to-a-deployed-rest.md (names these three check ids in its trace list but under different step keys), 20261029T193654Z-apigateway-lambda-api-gateway-lambda-16-integration-checks-never-complete.md, 20261102T090835Z-apigateway-dynamodb-lambda-logs-api-gateway-rest-proxy-integrations-never-reach-a.md and 20261029T235159Z-apigateway-dynamodb-lambda-2-causes-the-malformed-proxy.md.
Traces: apigateway__dynamodb__iam__lambda.granted-caller-reads-the-seeded-agreement-verbatim-through-the-lambda-proxy, apigateway__dynamodb__iam__lambda.agreement-rewritten-in-dynamodb-is-served-on-the-next-signed-get, apigateway__dynamodb__iam__lambda.removing-the-invoke-permission-is-rejected-with-a-500-at-the-stage.

## Evidence
Expected (recorded AWS): signed GET https://<api-id>.execute-api.<region>/<stage>/data-agreements/<id> -> HTTP 200, Content-Type: application/json, body = the handler's serialization of the DynamoDB item (every attribute verbatim); after UpdateItem rewrites the item, the next signed GET -> 200 with the rewritten attribute values; after lambda RemovePermission of the apigateway.amazonaws.com statement for that stage, the same signed GET -> HTTP 500 with x-amzn-ErrorType: InternalServerErrorException and body {"message": "Internal server error"}.

Actual (emulator): no response is ever returned. All three checks report exactly 'Error: Check timed out after 60.0s' from their _await poll of the stage invoke URL; the suite records no status, no headers and no body for any of the requests (granted GET, post-rewrite GET, post-RemovePermission GET).

## Steps to Reproduce
1. Create a REST API with resource /data-agreements/{agreementId} and a GET method with authorizationType=AWS_IAM, wired to an AWS_PROXY integration targeting a Lambda function that reads a DynamoDB table; grant apigateway.amazonaws.com lambda:InvokeFunction for the stage via AddPermission; create a deployment and the stage.
2. Seed one item in the table, assume a role whose policy allows execute-api:Invoke on <api-arn>/<stage>/GET/*, and send a SigV4-signed GET to the stage invoke URL. Observe that no HTTP response arrives within 60 s (AWS answers 200 with the item as application/json).
3. UpdateItem to change the item's version attribute and add a list attribute, then repeat the signed GET: no response arrives (AWS serves the rewritten item).
4. Call lambda RemovePermission for the stage's apigateway statement and repeat the signed GET: no response arrives (AWS answers 500 x-amzn-ErrorType: InternalServerErrorException, body {"message": "Internal server error"}).
5. Or replay the three check ids listed in trace_ids.
