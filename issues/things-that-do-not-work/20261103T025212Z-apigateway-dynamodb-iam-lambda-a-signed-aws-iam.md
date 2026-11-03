Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__iam__lambda.granted-caller-reads-the-seeded-agreement-verbatim-through-the-lambda-proxy
Issue-Key: apigateway__dynamodb__iam__lambda.granted-caller-reads-the-seeded-agreement-verbatim-through-the-lambda-proxy|_await#5|Timeout|last = probe()
Issue-Commit: 1a8bd0a23e8abbb897c29ab819f026e65ce436bb

# apigateway__dynamodb__iam__lambda: a signed AWS_IAM GET on a deployed REST stage with an AWS_PROXY Lambda integration never produces any HTTP response, so the verbatim proxy-passthrough body and the handler's own 404 are unobservable

> _Note: this issue was identified against the code base at commit `1a8bd0a23e8abbb897c29ab819f026e65ce436bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api deployed stage, AWS_PROXY Lambda-proxy integration)

## Description
This issue carries 1 net-new root cause.

Cause 1 of 1: a SigV4-signed GET to a deployed REST API stage whose only method is AWS_IAM-authorized and wired to an AWS_PROXY (Lambda proxy) integration over a DynamoDB-reading handler never produces an HTTP response at all — neither the success passthrough nor the handler's not-found body.
Failing check ids for this cause: apigateway__dynamodb__iam__lambda.granted-caller-reads-the-seeded-agreement-verbatim-through-the-lambda-proxy.

AWS: recorded behavior only. The execute-api data plane is not described by the API Gateway Smithy model — the model covers only the control-plane setup this check performs (CreateRestApi with endpointConfiguration REGIONAL, CreateResource, PutMethod with authorizationType=AWS_IAM, PutIntegration with type=AWS_PROXY and integrationHttpMethod=POST targeting arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fn-arn>/invocations, CreateDeployment with a stageName), plus lambda AddPermission for principal apigateway.amazonaws.com scoped to <execute-api-arn>/<stage>/GET/<resource-path>/*. What the recording shows of the data plane is that EVERY request reaching a deployed stage terminates in a response: (a) a SigV4-signed (service name execute-api) GET of a path the caller's role grants execute-api:Invoke on is answered HTTP 200 with response header Content-Type: application/json and a body that is byte-for-byte what the proxy handler returned — the DynamoDB item serialized verbatim, including nested map attributes and an embedded JSON-schema string that must not be re-encoded; (b) a request for a key that is not in the table is answered with the handler's own proxy response, HTTP 404 with body {"error": "Agreement not found"} (that message is the HANDLER's fixed text; the path, api id, stage and item id are values taken from the request and the table's state, never fixed).
Emulator: no HTTP response is produced on this path at any point. The check polls the stage invoke URL (two consecutive 200s required) until the harness budget is exhausted and the suite reports exactly 'Error: Check timed out after 60.0s'. No status line, no headers and no body are ever observed, so neither the 200 verbatim passthrough nor the 404 not-found body is ever served.
Why: a client of a proxy-integrated REST API hangs until its own socket or SDK timeout instead of receiving any status. It cannot tell a successful backend response from a backend 404, cannot rely on Content-Type: application/json, and cannot rely on the proxy contract that the handler's statusCode/headers/body are passed through unmodified (in particular that a JSON string attribute is not re-encoded). Any retry, caching or error-handling logic that keys on 200 vs 404 never runs.
Fix: the execute-api data-plane request path of the API Gateway service — resolution of a deployed stage and its resource/method, SigV4 verification and execute-api:Invoke authorization at the stage, the AWS_PROXY invoke of the target Lambda function, and the mapping of the handler's proxy response (statusCode, headers, body) back onto the HTTP response. Every request that reaches a deployed stage must terminate in a response.
Related open issue naming this same surface but NOT carrying this check's replay key as reported by this run (so the suite does not report the check as owned and it must be closed alongside): 20261103T014646Z-apigateway-dynamodb-iam-lambda-a-deployed-rest-stage.md. Further open issues describing the same surface: 20261102T091623Z-apigateway-18-further-requests-to-a-deployed-rest.md, 20261029T193654Z-apigateway-lambda-api-gateway-lambda-16-integration-checks-never-complete.md, 20261102T090835Z-apigateway-dynamodb-lambda-logs-api-gateway-rest-proxy-integrations-never-reach-a.md.
Traces: apigateway__dynamodb__iam__lambda.granted-caller-reads-the-seeded-agreement-verbatim-through-the-lambda-proxy.

## Evidence
Expected (recorded AWS): a SigV4-signed (execute-api) GET of https://<api-id>.execute-api.<region>.<suffix>/<stage>/data-agreements/<seeded-id> is answered HTTP 200 with Content-Type: application/json and a body equal, attribute for attribute, to the item put into the DynamoDB table — nested map attributes intact and the embedded JSON-schema string not re-encoded. The same signed GET for an id that was never seeded is answered HTTP 404 with body {"error": "Agreement not found"} (the handler's fixed text).

Actual (emulator): no response is ever returned on the stage invoke path. The check's poll loop re-issues the signed GET until the harness cap and the suite records exactly 'Error: Check timed out after 60.0s'. No status, no headers and no body are observed for either the granted GET or the unseeded-id GET; the assertions on content-type, on the verbatim body and on (404, {"error": "Agreement not found"}) are therefore never reached.

## Steps to Reproduce
1. Create a DynamoDB table (PAY_PER_REQUEST) keyed on a string hash attribute and put one item carrying a nested map attribute and a string attribute whose value is itself a JSON document.
2. Create an IAM role trusted by lambda.amazonaws.com with read access to that table, and a python3.12 Lambda function whose handler reads the item named by event['pathParameters'] and returns {statusCode: 200, headers: {Content-Type: application/json}, body: json.dumps(item)} or, when the key is absent, {statusCode: 404, body: json.dumps({'error': 'Agreement not found'})}.
3. Create a REGIONAL REST API with resource /data-agreements/{agreementId}; PutMethod GET with authorizationType=AWS_IAM; PutIntegration type=AWS_PROXY, integrationHttpMethod=POST, uri=arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fn-arn>/invocations.
4. lambda AddPermission for Principal apigateway.amazonaws.com, Action lambda:InvokeFunction, SourceArn arn:aws:execute-api:<region>:<account>:<api-id>/<stage>/GET/data-agreements/*; then CreateDeployment with that stageName.
5. Create a role whose inline policy allows execute-api:Invoke on arn:aws:execute-api:<region>:<account>:<api-id>/<stage>/GET/*, assume it, and send a SigV4-signed (service execute-api) GET to <stage-invoke-url>/data-agreements/<seeded-id>. Observe that no HTTP response arrives within the budget; AWS answers 200 application/json with the item verbatim.
6. Repeat with an id that was never seeded. Observe that no HTTP response arrives; AWS answers 404 {"error": "Agreement not found"}.
7. Or replay the check id listed in trace_ids.
