Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda.apigw-lambda-proxy-malformed-response-bad-gateway apigateway__dynamodb__lambda.apigw-stage-variables-isolate-ddb-target-per-stage
Issue-Key: apigateway__dynamodb__lambda.apigw-lambda-proxy-malformed-response-bad-gateway|_poll_http#10|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda.apigw-stage-variables-isolate-ddb-target-per-stage|_poll_http#13|Timeout|time.sleep(interval_s)
Issue-Commit: 54073c01592ce4e56a3cca58cb5387afed010108

# apigateway__dynamodb__lambda: 2 causes — the malformed-proxy-response 502 and per-stage stageVariables are unobservable because the deployed stage never answers (2 causes)

> _Note: this issue was identified against the code base at commit `54073c01592ce4e56a3cca58cb5387afed010108`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutIntegration

## Description
Two net-new root causes on the API Gateway REST execute-api data plane in front of Lambda. Neither failing check is marked ALREADY FILED or OWNED by the replay report. The open issues 20261025T081755Z-apigateway-dynamodb-lambda-a-lambda-proxy-integration-answers.md, 20261027T101902Z-apigateway-dynamodb-lambda-the-request-validator-negative-check-fa24e3.md and 20261025T043706Z-apigateway-dynamodb-lambda-an-apikeyrequired-method-behind-a.md describe neighbouring symptoms on this surface but carry other checks' replay keys and do not cover either cause below.

Cause 1 of 2: a Lambda AWS_PROXY integration whose handler returns a payload with no `statusCode` must make the stage answer 502 `Internal server error`; the emulator's stage never answers at all.
AWS: Smithy — PutIntegration.type is the IntegrationType enum and takes AWS_PROXY (services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json); the execute-api data plane itself is not modelled, so recorded behavior only: for an AWS_PROXY integration API Gateway invokes the function and translates the returned {statusCode, headers, body} envelope into the HTTP response. When the returned object is not that envelope (here: a dict with only `message`, no `statusCode`), the translation fails and API Gateway answers HTTP 502 with the fixed JSON body {"message":"Internal server error"} — the whole body is fixed text for this error class, there are no request- or state-derived parts. The function has already run, so its side effect (a DynamoDB PutItem of the marker item) is committed and observable. A control route on the same stage whose handler returns a well-formed envelope answers 200 with the handler's body.
Emulator: no response is produced for either route. The check polls the stage invoke URL (/_aws/execute-api/<api-id>/test/good, then /bad) and is killed by the harness at its 60 s cap — reported as `Error: Check timed out after 60.0s`. No status, no body, and therefore neither the 200 control nor the 502 refusal is ever observed.
Why: a client gets no answer and no error instead of either the handler's 200 or the gateway's 502. The only boundary that distinguishes "the integration was never invoked" from "the integration ran and its response failed translation" is unobservable, so an over-permissive pass-through of a malformed handler payload cannot be detected; SDK retry/timeout behaviour bears no resemblance to AWS.
Fix: the execute-api data plane's AWS_PROXY invocation path — the proxy-event construction, the Lambda invocation, and the response-envelope translation must terminate the request with a response (200 from a well-formed envelope, 502 + {"message":"Internal server error"} when `statusCode` is absent) rather than leaving the connection open.
Traces: apigateway__dynamodb__lambda.apigw-lambda-proxy-malformed-response-bad-gateway.

Cause 2 of 2: per-stage `variables` are never threaded into the proxy invoke event, because two stages of one deployment never answer at their invoke URLs.
AWS: Smithy — CreateStage.variables is a MapOfStringToString on the stage resource (same model, output shape Stage echoes `variables`). Recorded behavior only for the data plane: two stages created from the SAME deploymentId and differing only in their `variables` map each deliver their own map to the integration as `event.stageVariables`, and `event.requestContext.stage` is that stage's name; the same Lambda therefore writes a different DynamoDB item per stage, and no invoke arrives with an absent stageVariables map. Each stage answers HTTP 200 with the handler's JSON body.
Emulator: neither stage's invoke URL (/_aws/execute-api/<api-id>/alpha/put, .../beta/put) produces any response; the check is killed at the 60 s harness cap — `Error: Check timed out after 60.0s`. No 200, no body, and no DynamoDB item is written via either stage.
Why: stageVariables is the mechanism by which one deployment is promoted across environments, and it exists only in the invoke event — a control-plane CreateStage(variables=...) round-trip proves nothing. With the stage never answering, a client cannot promote a deployment at all, and an emulator that stored the map but dropped it from the proxy event (or leaked one stage's variables into another) is indistinguishable from one that threaded it correctly.
Why (shared): both causes are the same surface failing to produce any HTTP response; they are listed separately because they are two distinct requirements (response-envelope translation / 502 refusal, and per-stage variable threading) and a fix for one does not imply the other.
Fix: the execute-api data plane's request handling for a deployed stage — stage resolution from the URL's stage segment, population of `stageVariables` and `requestContext.stage` in the proxy event from that stage's stored `variables`, and bounded termination of the request with a response.
Traces: apigateway__dynamodb__lambda.apigw-stage-variables-isolate-ddb-target-per-stage.

## Evidence
Cause 1 — check apigateway__dynamodb__lambda.apigw-lambda-proxy-malformed-response-bad-gateway (verdict: mismatch):
Error: Check timed out after 60.0s
Setup performed by the check: DynamoDB table; Lambda `bad` whose handler PutItem's a marker row and then returns {"message": "this is not a proxy response"} (no statusCode); Lambda `good` returning {"statusCode":200, "headers":{"Content-Type":"application/json"}, "body":"{\"ok\":true}"}; one REST API with resources /good and /bad, GET methods, PutIntegration type=AWS_PROXY integrationHttpMethod=POST to each function, AddPermission for apigateway.amazonaws.com on arn:aws:execute-api:<region>:<account>:<api-id>/*/*, then a deployment and stage `test`.
Expected (AWS): GET /good -> 200 with body {"ok": true}; GET /bad -> 502 with body {"message":"Internal server error"} (fixed text), and the marker item present in DynamoDB proving the handler ran.
Actual (emulator): neither request ever reaches an accepted status; the harness kills the check at 60 s with no status recorded for either route.

Cause 2 — check apigateway__dynamodb__lambda.apigw-stage-variables-isolate-ddb-target-per-stage (verdict: mismatch):
Error: Check timed out after 60.0s
Setup performed by the check: DynamoDB table; one Lambda whose handler reads event["stageVariables"]["marker"] (defaulting to the sentinel "STAGE-VARIABLES-MISSING") and requestContext.stage, PutItem's {pk: marker, stage: stage, marker: marker} and returns {"statusCode":200, body: {"marker":..., "stage":...}}; one REST API, GET /put with PutIntegration type=AWS_PROXY to that function; ONE CreateDeployment; two CreateStage calls (`alpha`, `beta`) from that deploymentId differing only in variables={"marker": <per-stage marker>}.
Expected (AWS): GET on each stage's invoke URL -> 200 with body.marker == that stage's marker and body.stage == that stage name; two distinct DynamoDB items, each carrying its own stage; no item keyed "STAGE-VARIABLES-MISSING".
Actual (emulator): neither stage's invoke URL answers within the 60 s cap; no status, no body, no items written.

## Steps to Reproduce
Cause 1:
1. CreateTable in DynamoDB. CreateFunction `bad` with a handler that PutItem's a marker row and returns {"message": "this is not a proxy response"} (deliberately no statusCode). CreateFunction `good` returning a well-formed {"statusCode":200, "headers":{...}, "body": "..."} envelope.
2. CreateRestApi; CreateResource pathPart=good and pathPart=bad; PutMethod GET authorizationType=NONE on each; PutIntegration type=AWS_PROXY integrationHttpMethod=POST uri=arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fn-arn>/invocations on each; lambda AddPermission for principal apigateway.amazonaws.com with SourceArn arn:aws:execute-api:<region>:<account>:<api-id>/*/*.
3. CreateDeployment + CreateStage stageName=test.
4. GET the stage invoke URL for /good: expected 200 {"ok": true}. Observe that the request never completes.
5. GET the stage invoke URL for /bad: expected 502 with body {"message":"Internal server error"} and the marker item present in the table. Observe that the request never completes.

Cause 2:
1. CreateTable; CreateFunction with a handler that echoes event["stageVariables"]["marker"] and event["requestContext"]["stage"] into both a DynamoDB PutItem and a 200 proxy response.
2. CreateRestApi; CreateResource pathPart=put; PutMethod GET authorizationType=NONE; PutIntegration type=AWS_PROXY to that function; AddPermission as above.
3. CreateDeployment once; CreateStage stageName=alpha variables={"marker":"alpha-<id>"} and CreateStage stageName=beta variables={"marker":"beta-<id>"} from the SAME deploymentId.
4. GET /put on each stage's invoke URL. Expected: 200 whose body.marker equals that stage's marker and body.stage equals the stage name, plus one DynamoDB item per stage and no item keyed STAGE-VARIABLES-MISSING. Observe that neither request completes within 60 s.
