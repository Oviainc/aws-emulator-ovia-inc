Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.apigw-http-api-lambda-integration-positive apigateway__dynamodb__lambda__logs.apigw-websocket-api-lambda-integration-positive apigateway__dynamodb__lambda__logs.apigw-websocket-connect-disconnect-logged apigateway__dynamodb__lambda__logs.iam-role-trusts-apigateway-service-positive apigateway__dynamodb__lambda__logs.iam-role-wrong-trust-principal-apigateway-negative
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-http-api-lambda-integration-positive|_poll_http_get#15|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-websocket-api-lambda-integration-positive|_poll_ws_connect#20|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-websocket-connect-disconnect-logged|_poll_ws_connect#20|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.iam-role-trusts-apigateway-service-positive|_poll_http_get#19|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.iam-role-wrong-trust-principal-apigateway-negative|_poll_http_get_until_stable#27|Timeout|time.sleep(interval_s)
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# apigateway__dynamodb__lambda__logs: five checks hang to the 60 s cap — HTTP-API and WebSocket-API Lambda integrations and the apigateway-trust-policy legs never get a response

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: a request driven at a deployed API Gateway endpoint never returns.
AWS: a deployed HTTP API with an AWS_PROXY Lambda integration answers the invoke URL with the function's response (200 for these positive legs); a WebSocket API completes the $connect handshake and logs $connect/$disconnect; a stage whose execution role trusts apigateway.amazonaws.com invokes the integration (200) while a role with the wrong trust principal makes the integration fail (5xx). Recorded behavior only for the per-case statuses; what is structural is that every request to a deployed endpoint produces an HTTP response (or a completed/refused WebSocket handshake).
Emulator: no response arrives for any of the five. Each exhausts the harness budget and is reported as "Check timed out after 60.0s".
Why: the client stalls rather than succeeding or being refused, so neither the positive integration path nor the trust-policy negative can be distinguished, and the CloudWatch log assertions that depend on traffic having flowed can never run.
Fix: the execute-api data-plane request path (REST, HTTP-API and WebSocket stage dispatch) — it must always produce a response.
Traces: apigateway__dynamodb__lambda__logs.apigw-http-api-lambda-integration-positive, apigateway__dynamodb__lambda__logs.apigw-websocket-api-lambda-integration-positive, apigateway__dynamodb__lambda__logs.apigw-websocket-connect-disconnect-logged, apigateway__dynamodb__lambda__logs.iam-role-trusts-apigateway-service-positive, apigateway__dynamodb__lambda__logs.iam-role-wrong-trust-principal-apigateway-negative.

## Evidence
Detail for all five checks is exactly "Error: Check timed out after 60.0s" — no HTTP status line, no WebSocket handshake result and no body ever arrives.

## Steps to Reproduce
1. Deploy an HTTP API (and a WebSocket API) with an AWS_PROXY Lambda integration and a stage.
2. Request the stage invoke URL (and open a WebSocket connection to it) and observe that nothing is returned within 60 s.
3. Or replay the check ids in trace_ids.
