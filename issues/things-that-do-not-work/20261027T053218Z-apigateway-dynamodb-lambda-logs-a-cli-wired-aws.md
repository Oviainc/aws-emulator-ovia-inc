Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.cli-apigateway-lambda-integration-setup-and-test
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-lambda-integration-setup-and-test|run#27|AssertionError|assert status == 200, f"unexpected status {status}: {body_text}"
Issue-Commit: a55c69b9a776d6a2a8a52ff1d4d449819274859a

# apigateway__dynamodb__lambda__logs: a CLI-wired AWS_PROXY Lambda integration on a deployed REST stage answers 500 Internal server error instead of the handler's response (1 cause)

> _Note: this issue was identified against the code base at commit `a55c69b9a776d6a2a8a52ff1d4d449819274859a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutIntegration

## Description
This issue carries 1 root cause.

=== Cause 1 of 1: the execute-api data plane never runs an AWS_PROXY integration — the deployed stage answers 500 Internal server error ===
AWS: recorded behavior only for the data plane (the execute-api invoke endpoint is not part of the apigateway Smithy model; the control-plane calls the check makes — CreateRestApi, CreateResource, PutMethod, PutIntegration(type=AWS_PROXY, integrationHttpMethod=POST, uri=arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<function-arn>/invocations), CreateDeployment, and lambda AddPermission for principal apigateway.amazonaws.com — all succeed and the GetIntegration read-back is already correct). Recorded, a POST of {"text":"hello"} to https://<api-id>.execute-api.<region>.amazonaws.com/live/process returns HTTP 200 with the handler's proxy response body {"upper":"HELLO"} (Content-Type application/json), and the handler's print lands in the /aws/lambda/<function> log group.
Emulator: every POST to the deployed stage path answers HTTP 400-class/500 — measured, status 500 with body {"message":"Internal server error"} for the full 45 s the check polls. The integration is therefore never executed: no proxy response and no log line.
Why: 500 Internal server error is API Gateway's own body for an integration that could not be invoked, so a client cannot tell a broken deployment from a broken handler; any application fronted by a Lambda proxy integration is unusable, and the CLI-driven setup path reports success at every step while the endpoint it produced serves nothing.
Fix: the execute-api data-plane request path for REST APIs — resolving the deployed stage's method/integration and invoking the AWS_PROXY target, then mapping the handler's statusCode/headers/body back to the HTTP response (the apigateway crate's data-plane dispatch plus its Lambda invoke seam). Related open issues that describe the same symptom for other service groups and do NOT carry this check: 20261025T081755Z-apigateway-dynamodb-lambda-a-lambda-proxy-integration-answers.md and 20261025T014422Z-apigateway-api-gateway-a-deployed-rest-api-stage-serves.md.
Traces: apigateway__dynamodb__lambda__logs.cli-apigateway-lambda-integration-setup-and-test.

## Evidence
Check apigateway__dynamodb__lambda__logs.cli-apigateway-lambda-integration-setup-and-test, at the assertion on the deployed invoke URL:
  AssertionError: unexpected status 500: {"message":"Internal server error"}
  (check line 94: assert status == 200, f"unexpected status {status}: {body_text}")
Expected: HTTP 200 with body {"upper": "HELLO"}, followed by the marker 'PROCESSED:hello' appearing in the /aws/lambda/<function-name> log group.
All preceding legs passed: create-rest-api, get-resources, create-resource, put-method, put-integration (AWS_PROXY), the SDK GetIntegration read-back (type == AWS_PROXY and uri == the Lambda invoke ARN), lambda add_permission for apigateway.amazonaws.com with SourceArn arn:aws:execute-api:<region>:<account>:<api-id>/*/POST/process, and create-deployment returning a deployment id.

## Steps to Reproduce
1. Create a python3.12 Lambda that reads event['body'], prints 'PROCESSED:<text>' and returns {'statusCode':200,'headers':{'Content-Type':'application/json'},'body':json.dumps({'upper': text.upper()})}; wait for State == Active.
2. aws apigateway create-rest-api; get-resources to find '/'; create-resource --path-part process; put-method --http-method POST --authorization-type NONE; put-integration --type AWS_PROXY --integration-http-method POST --uri arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fn-arn>/invocations.
3. lambda add-permission for principal apigateway.amazonaws.com with SourceArn arn:aws:execute-api:<region>:<account>:<api-id>/*/POST/process; aws apigateway create-deployment --stage-name live.
4. POST {"text":"hello"} to <stage invoke url>/process. Observed: HTTP 500 {"message":"Internal server error"} (repeated for 45 s). Expected: HTTP 200 {"upper":"HELLO"} and the 'PROCESSED:hello' line in /aws/lambda/<fn-name>.
