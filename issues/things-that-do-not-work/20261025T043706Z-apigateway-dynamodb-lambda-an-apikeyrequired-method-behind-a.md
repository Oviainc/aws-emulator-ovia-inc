Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda.apigw-missing-api-key-not-delivered-to-integration
Issue-Key: apigateway__dynamodb__lambda.apigw-missing-api-key-not-delivered-to-integration|_poll_http#20|Timeout|time.sleep(interval_s)
Issue-Commit: 06b7d43ec7937dbf787f77738b3813bde2a779f1

# apigateway__dynamodb__lambda: an apiKeyRequired method behind a usage plan never answers at the deployed stage invoke URL, so the keyless-403 negative hangs to the 60 s cap

> _Note: this issue was identified against the code base at commit `06b7d43ec7937dbf787f77738b3813bde2a779f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
One root cause. Not covered by any open issue's replay key: the adjacent open issue 20261024T043741Z-apigateway-dynamodb-lambda-the-api-gateway-lambda-dynamodb.md describes the same family of symptoms (404 UnknownService at a stage invoke URL, and a 60 s hang) but carries only the keys for apigw-lambda-ddb-pipeline-positive and apigw-proxy-invoke-denied-without-lambda-resource-policy-for-that-path, so this check's cause is unowned. 20261023T210711Z-apigateway-api-gateway-data-plane-is-dead-30-checks.md is likewise an adjacent cause for other keys.

Cause 1 of 1: a deployed REST API stage whose method has apiKeyRequired=true and is wired to an AWS_PROXY Lambda integration never produces an HTTP response at its invoke URL, so neither the keyed control nor the keyless rejection can be observed.
AWS: Smithy (API Gateway control plane): PutMethod.apiKeyRequired is a Boolean (default false) on PUT /restapis/{restApiId}/resources/{resourceId}/methods/{httpMethod}; CreateDeployment is POST /restapis/{restApiId}/deployments, code 201. The execute-api data plane is not in the control-plane model, so for the invoke itself this is recorded behavior only: against AWS a GET at the deployed stage's invoke URL carrying a valid x-api-key that is attached to a usage plan covering (apiId, stage) returns the Lambda's own response — HTTP 200 with the handler's JSON body — and the item the handler wrote is then readable from DynamoDB; the same GET with the x-api-key header removed is refused by API Gateway itself with HTTP 403 and the integration is never invoked, so no item appears for that id. In both directions a status line arrives promptly.
Emulator: no HTTP response at all. The check reports exactly 'Check timed out after 60.0s' while polling the keyed control request for a 200 (its own budget is 180 s), i.e. the stage invoke URL never serves the integration and the request neither succeeds nor fails.
Why: a client gets a hang instead of a status. 200-vs-403 is the whole API-key/usage-plan boundary; because no response is ever produced, an emulator user cannot tell 'rejected before the backend' from 'rejected but the backend still ran', and every monetisation/quota boundary built on apiKeyRequired is unverifiable. A hang also stalls callers and waiters rather than failing them fast.
Fix: the emulator's execute-api data-plane request path for a deployed stage — it must always produce a response — plus the API-key/usage-plan admission check that must run before the AWS_PROXY integration is invoked. Candidate components: the front-door request routing that classifies the execute-api stage endpoint, and the API Gateway stage-invoke handler in services/service-com-amazonaws-apigateway-backplanecontrolservice.
Traces: apigateway__dynamodb__lambda.apigw-missing-api-key-not-delivered-to-integration.

## Evidence
Expected (AWS, recorded): GET <stage invoke url>/items?id=<id> with a valid x-api-key -> HTTP 200, body {"stored": "<id>"}, and the DynamoDB item for <id> becomes readable; the same GET without x-api-key -> HTTP 403 and no item is ever written for that id.
Actual (emulator): no response. Suite verdict [mismatch], detail: 'Error: Check timed out after 60.0s' — the control poll for a 200 at the stage invoke URL never completes, so the keyless 403 assertion and the no-item side-channel assertion are never reached.

## Steps to Reproduce
1. create_table (pk:S, PAY_PER_REQUEST) and create a Lambda whose handler put_items {pk: queryStringParameters['id'], source: 'apikey-route'} and returns 200 with body {"stored": <id>}.
2. create_rest_api -> create_resource(pathPart='items') -> put_method(httpMethod='GET', authorizationType='NONE', apiKeyRequired=True) -> put_integration(type='AWS_PROXY', integrationHttpMethod='POST', uri=arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fn arn>/invocations).
3. lambda.add_permission for apigateway.amazonaws.com with SourceArn arn:aws:execute-api:<region>:<account>:<apiId>/*/* ; create_deployment(stageName='test').
4. create_api_key(enabled=True), create_usage_plan(apiStages=[{apiId, stage:'test'}]), create_usage_plan_key(keyType='API_KEY').
5. GET <stage invoke url>/items?id=withkey-1 with header x-api-key: <key value> -> expect HTTP 200; observe that no response is ever returned and the call hangs until the check's 60 s cap.
6. Or replay the check id in trace_ids.
