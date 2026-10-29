Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda.apigw-missing-api-key-not-delivered-to-integration apigateway__dynamodb__lambda.apigw-request-validator-rejects-bad-body-integration-not-invoked
Issue-Key: apigateway__dynamodb__lambda.apigw-missing-api-key-not-delivered-to-integration|_http#20|Timeout|with urllib.request.urlopen(req, timeout=timeout_s) as resp:
Issue-Key: apigateway__dynamodb__lambda.apigw-request-validator-rejects-bad-body-integration-not-invoked|_http#18|Timeout|e.read().decode("utf-8", "replace"),
Issue-Commit: ef111eb3ea31ef4faca303fd106b7202052fa3ed

# apigateway__dynamodb__lambda: the deployed REST stage never serves an AWS_PROXY method, so neither the apiKeyRequired keyless-403 nor the request-validator 400 "Invalid request body" refusal can ever be observed (2 causes)

> _Note: this issue was identified against the code base at commit `ef111eb3ea31ef4faca303fd106b7202052fa3ed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMethod

## Description
Two net-new root causes on the API Gateway REST execute-api data plane in front of a Lambda AWS_PROXY integration. Neither failing check is marked ALREADY FILED or OWNED in this replay. Adjacent open issues describe the same family but do not carry these checks' replay keys and so do not cover them: 20261025T043706Z-apigateway-dynamodb-lambda-an-apikeyrequired-method-behind-a.md (key `_poll_http#20`), 20261027T101902Z-apigateway-dynamodb-lambda-the-request-validator-negative-check-fa24e3.md (key `_poll_http#18`), 20261025T081755Z-apigateway-dynamodb-lambda-a-lambda-proxy-integration-answers.md (proxy integration answers 500) and 20261025T014422Z-apigateway-api-gateway-a-deployed-rest-api-stage-serves.md. Both causes below are the deployed stage producing NO response at all.

Cause 1 of 2: a method with apiKeyRequired=true behind a usage plan never answers at the deployed stage invoke URL, so the API-key admission boundary is unobservable.
AWS: Smithy (apigateway control plane): PutMethod (PUT /restapis/{restApiId}/resources/{resourceId}/methods/{httpMethod}, code 201) takes apiKeyRequired: Boolean with default false; the execute-api data plane itself is not in the control-plane model, so for the invoke this is recorded behavior only. Against AWS, a GET at the deployed stage's invoke URL carrying an x-api-key that is attached, via CreateUsagePlanKey, to a usage plan whose apiStages cover (apiId, stage) is admitted and served by the AWS_PROXY integration: the handler's own envelope is returned, i.e. HTTP 200 with the handler's JSON body, and the item the handler wrote becomes readable in DynamoDB. The SAME request with the x-api-key header removed is refused by API Gateway before the integration runs: HTTP 403 with the gateway's Forbidden envelope — body {"message":"Forbidden"} (fixed text; AWS's MISSING_AUTHENTICATION_TOKEN/api-key gateway response) and header x-amzn-ErrorType: ForbiddenException — and no item is written for that request's id. In both directions a status line arrives promptly. Fixed text here is the word Forbidden; the only request-derived values are the path, the query id and the key value.
Emulator: no HTTP response is ever produced. The check is killed by the harness at its 60 s cap ("Error: Check timed out after 60.0s") while polling the KEYED control request for a 200 — its own poll budget is 180 s — so the stage invoke URL neither succeeds nor fails and the keyless leg is never reached.
Why: a client gets a hang instead of a status. 200-vs-403 is the entire API-key/usage-plan boundary, so a user cannot distinguish "refused before the backend" from "refused but the backend still ran", and every metering/monetisation path built on apiKeyRequired is unverifiable. A hang is worse than a wrong status: SDK timeouts and retries behave nothing like AWS.
Fix: the execute-api data-plane request path for a deployed stage (it must always terminate with a response) plus the API-key/usage-plan admission step that must run BEFORE the AWS_PROXY integration is dispatched — the stage-invoke/front-door routing of the apigateway service crate, not the control-plane handlers, which already accept the wiring.
Traces: apigateway__dynamodb__lambda.apigw-missing-api-key-not-delivered-to-integration.

Cause 2 of 2: a method with a request validator and JSON-Schema model never answers at the deployed stage, so the pre-invocation body refusal is unobservable.
AWS: Smithy: CreateRequestValidator (POST /restapis/{restApiId}/requestvalidators, code 201) takes validateRequestBody / validateRequestParameters, both Boolean default false, and PutMethod binds it through requestValidatorId plus requestModels. Recorded behavior only for the invoke: a schema-valid POST at the deployed stage is served by the AWS_PROXY integration with HTTP 200 and the handler's own body, while a body missing a property the model marks required is refused by API Gateway itself with HTTP 400 and the BAD_REQUEST_BODY gateway response — body {"message":"Invalid request body"} (fixed text) with header x-amzn-ErrorType: BadRequestException — and the function is NOT invoked, so no item appears under the handler's sentinel key. The only request-derived parts are the path and the body.
Emulator: no HTTP response is ever produced. The check is killed at the harness's 60 s cap ("Error: Check timed out after 60.0s") on its control POST of a schema-valid body, so neither the 200 control nor the 400 refusal nor the not-invoked side-channel is ever reached.
Why: the check belongs to the overly-permissive class — an unvalidated body reaching the integration is the failure being hunted — and a hang makes that failure neither demonstrable nor excludable. Clients relying on API Gateway to screen malformed input before the backend runs get no signal at all.
Fix: the execute-api data-plane integration dispatch for AWS_PROXY (it must produce the function's response at the stage and must always answer) plus the request-validation step that must evaluate requestValidatorId/requestModels and refuse with the BAD_REQUEST_BODY envelope before dispatch.
Traces: apigateway__dynamodb__lambda.apigw-request-validator-rejects-bad-body-integration-not-invoked.

## Evidence
Cause 1 — check apigateway__dynamodb__lambda.apigw-missing-api-key-not-delivered-to-integration, verdict [mismatch]:
  Error: Check timed out after 60.0s
  Expected (AWS): GET <stage-invoke-url>/items?id=<id> with a valid x-api-key attached to a usage plan covering (apiId, stage) -> HTTP 200 with the handler's JSON body {"stored": "<id>"}, and the DynamoDB item for <id> becomes readable; the same GET with the x-api-key header removed -> HTTP 403 with {"message":"Forbidden"} / x-amzn-ErrorType: ForbiddenException and no item ever written for that id.
  Actual (emulator): no status line at all — the control poll (_poll_http, accept = status == 200, budget 180 s) never completes and the check is cut at the suite's 60 s cap.

Cause 2 — check apigateway__dynamodb__lambda.apigw-request-validator-rejects-bad-body-integration-not-invoked, verdict [mismatch]:
  Error: Check timed out after 60.0s
  Expected (AWS): POST <stage-invoke-url>/orders with {"id": "<id>", "note": "valid"} -> HTTP 200 with the handler's {"stored": "<id>"}, item readable in DynamoDB; POST with {"note": "no id here"} (missing the model's required "id") -> HTTP 400 with {"message":"Invalid request body"} / x-amzn-ErrorType: BadRequestException, and NO DynamoDB item under the handler's sentinel key because the function was never invoked.
  Actual (emulator): no status line at all — the control POST poll never completes and the check is cut at the 60 s cap.

In both causes the control-plane calls (CreateRestApi, CreateResource, PutMethod, PutIntegration, CreateModel, CreateRequestValidator, AddPermission, CreateDeployment, CreateApiKey, CreateUsagePlan, CreateUsagePlanKey) succeeded; only the deployed execute-api endpoint failed to answer. (Host/port, generated api ids, key values and item ids are per-run values and are not part of the requirement.)

## Steps to Reproduce
Cause 1 (apiKeyRequired):
1. CreateTable (pk:S, PAY_PER_REQUEST) and wait ACTIVE. CreateFunction whose handler PutItem's {pk: queryStringParameters['id']} and returns {"statusCode":200, "body": {"stored": <id>}}.
2. CreateRestApi -> CreateResource(pathPart='items') -> PutMethod(httpMethod='GET', authorizationType='NONE', apiKeyRequired=True) -> PutIntegration(type='AWS_PROXY', integrationHttpMethod='POST', uri=arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fn-arn>/invocations).
3. lambda AddPermission for apigateway.amazonaws.com with SourceArn arn:aws:execute-api:<region>:<account>:<apiId>/*/*; CreateDeployment + stage 'test'.
4. CreateApiKey(enabled=True); CreateUsagePlan(apiStages=[{apiId, stage:'test'}]); CreateUsagePlanKey(keyType='API_KEY').
5. GET <stage-invoke-url>/items?id=withkey-1 with header x-api-key: <key value>. Expect HTTP 200 promptly; observe that no response is ever returned.
6. Then GET the same URL without x-api-key and expect HTTP 403 {"message":"Forbidden"} with no item written.

Cause 2 (request validator):
1. CreateTable + a Lambda whose handler falls back to a SENTINEL env value when the JSON body has no "id", PutItem's it and returns {"statusCode":200, "body": {"stored": <id>}}.
2. CreateRestApi; CreateModel(contentType='application/json') with a draft-04 schema requiring ["id","note"] and additionalProperties:false; CreateRequestValidator(validateRequestBody=True, validateRequestParameters=False); CreateResource(pathPart='orders'); PutMethod(POST, authorizationType='NONE', requestValidatorId=<id>, requestModels={'application/json': <model>}); PutIntegration type=AWS_PROXY at the function; AddPermission; CreateDeployment + stage 'test'.
3. POST <stage-invoke-url>/orders with {"id":"valid-1","note":"valid"} and Content-Type: application/json. Expect HTTP 200 with the handler's body; observe that no response is ever returned.
4. POST the same URL with {"note":"no id here"} and expect HTTP 400 {"message":"Invalid request body"} (x-amzn-ErrorType: BadRequestException) and no DynamoDB item under the sentinel key.

Or replay the two check ids in trace_ids.
