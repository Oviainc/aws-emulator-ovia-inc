Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda.apigw-lambda-ddb-pipeline-positive apigateway__dynamodb__lambda.apigw-proxy-invoke-denied-without-lambda-resource-policy-for-that-path
Issue-Key: apigateway__dynamodb__lambda.apigw-lambda-ddb-pipeline-positive|run#12|AssertionError|assert status == 200, f"Expected 200, got {status}: {resp_body}"
Issue-Key: apigateway__dynamodb__lambda.apigw-proxy-invoke-denied-without-lambda-resource-policy-for-that-path|_poll_until#26|Timeout|time.sleep(interval_s)
Issue-Commit: ffece56e68e804f0450b8a83661b5d243139afbd

# apigateway__dynamodb__lambda: a Lambda-proxy integration answers 500 Internal server error instead of the handler's result, and the per-path resource-policy negative never completes (2 causes)

> _Note: this issue was identified against the code base at commit `ffece56e68e804f0450b8a83661b5d243139afbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

_Status update:_ both causes are addressed on the `execute-api` data plane. An `AWS_PROXY`
integration is now PERFORMED — the method request is handed to the function as one proxy
event, the envelope the function returned is the method response, a return value that is no
envelope is answered `502`, and the invocation is made only where the function's own
resource-based policy grants `apigateway.amazonaws.com` `lambda:InvokeFunction` for the ARN of
the method being invoked (`src/service/invoke/lambda_proxy.rs`). The wait for the function's
answer is the integration's own `timeoutInMillis`, after which the call is answered through the
api's `INTEGRATION_TIMEOUT` gateway response, so a request to a deployed stage always
terminates. `tests/all/stage_lambda_proxy_integration.rs` drives the whole path end to end
across API Gateway, AWS Lambda and Amazon DynamoDB.

The `apigw-proxy-invoke-denied-without-lambda-resource-policy-for-that-path` replay key PASSES.
The `apigw-lambda-ddb-pipeline-positive` key's own assertion (`assert status == 200`) no longer
fires — the stage serves that call `200` with the handler's body and the item it wrote is
readable from DynamoDB — but the check is still killed at the harness's 60 s cap inside its own
`_poll_until` helper, on a step after that assertion which the replay could not place and which
is not in the `AWS_PROXY` path (the verdict is identical under two independent implementations
of that path). Whatever that poll waits for is unowned by this report.

## Operation
PutIntegration

## Description
Two net-new causes on the API Gateway REST data plane in front of Lambda. Neither failing check is marked ALREADY FILED or OWNED; the existing open issues 20261025T043706Z-apigateway-dynamodb-lambda-an-apikeyrequired-method-behind-a.md and 20261023T170254Z-apigateway-dynamodb-lambda-the-request-validator-negative-check.md cover different checks of this group and do not carry these replay keys.

Cause 1 of 2: AWS_PROXY integration to a Lambda function returns 500 instead of the handler's response.
AWS: for a Lambda proxy (AWS_PROXY) integration, API Gateway invokes the function, reads the handler's returned {statusCode, headers, body} envelope, and serves exactly that status and body to the caller; a handler returning statusCode 200 produces HTTP 200 at the stage. Recorded behavior only (the execute-api data plane is not described by the control-plane Smithy model); API Gateway reserves 500 with body {"message":"Internal server error"} for a malformed or failed integration, not for a successful invocation.
Emulator: the deployed stage answers HTTP 500 with body {"message":"Internal server error"} for a request whose Lambda proxy integration should have produced 200 and the handler's JSON.
Why: every client of an API-Gateway-fronted Lambda sees a server error for a request that works on AWS. The whole proxy-integration happy path is unusable, and application code that distinguishes its own 5xx from the gateway's cannot be exercised at all.
Fix: the execute-api data plane's AWS_PROXY integration path — the construction of the proxy event handed to Lambda, the invocation, and the translation of the handler's response envelope into the HTTP response (the generic 500 is being produced in place of that translation).
Traces: apigateway__dynamodb__lambda.apigw-lambda-ddb-pipeline-positive.

Cause 2 of 2: the stage never answers for a proxy method whose Lambda has no resource policy for that path, so the refusal can never be observed.
AWS: when a method's AWS_PROXY integration targets a function whose resource policy does not permit apigateway.amazonaws.com to invoke it for THAT path/method ARN, API Gateway fails the integration and answers the caller promptly (recorded behavior only: a 500-class gateway response, with the invocation denied rather than performed). The requirement under test is that a bounded HTTP response arrives and the ungranted path is not invoked.
Emulator: the request to the deployed stage never completes — the check is killed by the harness at its 60 s cap, so no status is ever produced.
Why: a hang is worse than a wrong answer: a client gets no response and no error, SDK retries and timeouts behave nothing like AWS, and the authorization boundary between a granted path and an ungranted one is unobservable.
Fix: the execute-api data plane's integration-invocation path — the Lambda resource-policy (lambda:InvokeFunction / SourceArn per path+method) decision must be evaluated and must terminate the request with a response instead of leaving it open.
Traces: apigateway__dynamodb__lambda.apigw-proxy-invoke-denied-without-lambda-resource-policy-for-that-path.

## Evidence
Cause 1 — check apigateway__dynamodb__lambda.apigw-lambda-ddb-pipeline-positive:
AssertionError: Expected 200, got 500: {"message":"Internal server error"}
(raised at check line 161 after POSTing to the deployed stage URL of a REST API whose method has an AWS_PROXY integration to a Lambda that writes to DynamoDB)
Expected (AWS): HTTP 200 with the handler's JSON body.
Actual (emulator): HTTP 500 {"message":"Internal server error"}.

Cause 2 — check apigateway__dynamodb__lambda.apigw-proxy-invoke-denied-without-lambda-resource-policy-for-that-path:
Error: Check timed out after 60.0s
Expected (AWS): a bounded HTTP response from the stage (the denied invocation surfaces as a gateway 5xx) and no invocation of the ungranted path.
Actual (emulator): no response at all within the 60 s harness cap.

## Steps to Reproduce
Cause 1:
1. CreateFunction with a Python handler that PutItem's into a DynamoDB table and returns {"statusCode":200, "body": json}.
2. CreateRestApi, child resource, PutMethod, PutIntegration type=AWS_PROXY pointing at that function, AddPermission for apigateway.amazonaws.com, CreateDeployment + CreateStage.
3. POST a JSON body to the stage invoke URL for that path.
4. Observe HTTP 500 {"message":"Internal server error"} instead of 200 and the handler's body.

Cause 2:
1. Same setup, but grant the Lambda resource policy only for a DIFFERENT path/method ARN than the one invoked.
2. Send the request to the ungranted path on the deployed stage.
3. Observe that the request never completes (no status within 60 s) instead of a prompt gateway response.
