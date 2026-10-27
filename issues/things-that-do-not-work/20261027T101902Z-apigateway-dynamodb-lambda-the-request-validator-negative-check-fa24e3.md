Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda.apigw-request-validator-rejects-bad-body-integration-not-invoked
Issue-Key: apigateway__dynamodb__lambda.apigw-request-validator-rejects-bad-body-integration-not-invoked|_poll_http#18|Timeout|time.sleep(interval_s)
Issue-Commit: 3bfaf82953d93db9f40ee26ed0fb0b9c83dbff8e

# apigateway__dynamodb__lambda: the request-validator negative check never completes — a Lambda AWS_PROXY method at a deployed stage never serves the control request, so the 400 "Invalid request body" refusal before invocation is never exercised

> _Note: this issue was identified against the code base at commit `3bfaf82953d93db9f40ee26ed0fb0b9c83dbff8e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One root cause (1 of 1).
AWS: a REST API method with an AWS_PROXY (Lambda proxy) integration and a request validator declaring validateRequestBody=true over a JSON-Schema model serves a schema-valid POST with HTTP 200 carrying the function's own response payload (here the handler's {"stored": <id>} after a DynamoDB PutItem), and refuses a body missing the model's required property with HTTP 400 and API Gateway's BAD_REQUEST_BODY envelope ({"message":"Invalid request body"}, x-amzn-ErrorType: BadRequestException) WITHOUT invoking the function — so no sentinel item is ever written by an id-less request.
Emulator: the check never gets past its control leg and the harness cuts it at the 60 s cap (recorded: "Check timed out after 60.0s"; no response or assertion captured). Its control step polls POST <invoke-url>/orders with a valid body until it answers 200 and asserts the body contains the stored id, so no 200-with-function-output was ever produced. recorded behavior only for the mechanism; other checks in this same replay show the emulator refusing lambda Invoke with the unmodeled code NotImplemented and the message "...running its python3.12 deployment package is not implemented in this emulator: there is no in-process runtime for it, and no response payload is returned that the function did not produce" — Hypothesis: the stage's AWS_PROXY dispatch cannot obtain a backend answer for the same reason, so the control request never reaches 200.
Why: the whole cross-service capability is unusable — a caller cannot verify that API Gateway screens an invalid body before the function runs, cannot observe the function's own 200 payload through the stage, and cannot distinguish "validator refused it" from "the backend never ran". The check belongs to the overly-permissive group precisely because an id-less request reaching the integration (a sentinel row in DynamoDB) is the failure being hunted, and that can neither be demonstrated nor excluded here.
Fix: the execute-api data-plane integration dispatch for AWS_PROXY (it must produce the function's response at the stage) plus the request-validation step that must refuse the bad body before dispatch. Two open issues are adjacent and neither covers this check: 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md (lambda Invoke has no in-process runtime) and 20261023T170254Z-apigateway-dynamodb-lambda-the-request-validator-negative-check.md, which attributes this check's abort to apigateway CreateModel answering 501 — CreateModel now answers in this replay (other checks reach its validation messages), so that stated cause no longer explains the failure.
Traces: apigateway__dynamodb__lambda.apigw-request-validator-rejects-bad-body-integration-not-invoked

## Evidence
Replay verdict: [mismatch] apigateway__dynamodb__lambda.apigw-request-validator-rejects-bad-body-integration-not-invoked — "Error: Check timed out after 60.0s" (no HTTP response, no assertion message).
The check's own expectations, in order: _poll_http(POST <stage invoke url>/orders, accept = status == 200) then assert json.loads(text)["stored"] == <good id>; then POST with body {"note": "no id here"} expecting bad_status == 400 and "invalid request body" in the response text; then a negative window asserting no DynamoDB item under the SENTINEL key and that the control item still exists. None of these was reached.
For comparison, the same emulator answers lambda Invoke in this replay with: (NotImplemented) "Invoke resolved arn:aws:lambda:<region>:<account>:function:<name> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator: there is no in-process runtime for it, and no response payload is returned that the function did not produce." (the arn/name shown in any one run are per-run values).

## Steps to Reproduce
1. CreateTable (DynamoDB) and wait ACTIVE; create a Lambda function (python3.12) whose handler reads the request body, falls back to the SENTINEL env var when the body has no "id", PutItem's it and returns {"stored": <id>}; wait State=Active.
2. CreateRestApi; CreateModel with a JSON Schema requiring "id"; CreateRequestValidator validateRequestBody=true; PutMethod POST /orders with requestModels={"application/json": <model>} and that requestValidatorId; PutIntegration type=AWS_PROXY pointing at the function; grant lambda:InvokeFunction to apigateway for the method; CreateDeployment + CreateStage "test".
3. POST the stage invoke URL /orders with a schema-valid body: expect HTTP 200 whose body is the function's {"stored": <id>} (this is the leg that never completes today).
4. POST the same URL with {"note": "no id here"}: expect HTTP 400 and {"message":"Invalid request body"} with x-amzn-ErrorType: BadRequestException, and confirm no DynamoDB item exists under the sentinel key (the function must not have been invoked).
Then replay the check id in trace_ids.
