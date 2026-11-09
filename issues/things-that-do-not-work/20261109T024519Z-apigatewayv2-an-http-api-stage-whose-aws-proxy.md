Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-target-without-invoke-permission-rejected
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-target-without-invoke-permission-rejected|_wait_stable#27|Timeout|time.sleep(0.5)
Issue-Commit: ec8f6ec2805a2454e78a14c856d8e041d8e7cb40

# apigatewayv2: an HTTP API stage whose AWS_PROXY IntegrationUri resolves through ${stageVariables.*} never produces any HTTP response, so the ungranted-target 500 and the per-stage stage-variable resolution reach no verdict (1 cause)

> _Note: this issue was identified against the code base at commit `ec8f6ec2805a2454e78a14c856d8e041d8e7cb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeApi

## Description
One net-new root cause. The failing check is neither marked ALREADY FILED nor OWNED in this replay.

Cause 1 of 1: the HTTP execute-api data plane never answers a request to a stage whose AWS_PROXY integration target is resolved from a stage variable, so neither the refusal nor the positive control is reachable.

AWS: an HTTP API integration may embed a stage variable in its target. Smithy: CreateIntegrationRequest$IntegrationUri — "For a Lambda integration, specify the URI of a Lambda function"; CreateStageRequest$StageVariables — "A map that defines the stage variables for a Stage. Variable names can have alphanumeric and underscore characters, and the values must match [A-Za-z0-9-._~:/?#&=,]+". For an IntegrationUri of the form arn:aws:lambda:<region>:<account>:function:${stageVariables.<name>}, the gateway substitutes the value from the *invoked stage's* StageVariables, so two stages sharing one integration/route/deployment resolve to two different functions. Recorded behavior (the model does not state the data-plane statuses): when the resolved function carries no lambda:InvokeFunction resource-policy grant for Principal apigateway.amazonaws.com with a matching SourceArn (arn:aws:execute-api:<region>:<account>:<api-id>/*/*), the gateway fails closed with HTTP 500 and the body exactly {"message":"Internal Server Error"} — fixed text, no variable part — and the function is never invoked (no side effect reaches the backend). A sibling stage on the SAME deployment whose stage variable names a permitted function keeps answering 200 with that function's own response body, and that invocation's event carries stageVariables equal to that stage's own variable map and requestContext.stage equal to that stage's name. Adding the resource-policy grant alone (no API, route, integration or deployment change) makes the previously refused stage serve its own target with 200. The 500 refusal is repeatable, not a one-off.

Emulator: no stage ever answers. Both the ungranted stage's invoke URL and the permitted twin's invoke URL produce no HTTP response at all — the in-flight GET on the invoke URL never returns and the check is killed at the 60 s harness cap (verdict: mismatch, "Check timed out after 60.0s"). There is no status line, no body and no refusal; the positive control (the permitted stage serving 200 with its marker) never completes, so the 500 {"message":"Internal Server Error"} contract, the no-invocation guarantee, the per-stage stageVariables/requestContext.stage projection and the repair-by-grant transition are all unobservable.

Related but not covering: open issue 20261107T125951Z-apigatewayv2-7-net-new-root-causes-where-the.md names this check under its Cause 2 but does not carry this replay key, and this replay did not mark the check as owned by it; open issue 20261024T043741Z-apigateway-lambda-the-stage-variable-integration-check-hangs.md is the REST API Gateway (service apigateway) analogue, a different service's data plane.

Why: a client that points an integration at ${stageVariables.*} gets a hung socket instead of a status. It can neither rely on the documented fail-closed 500 when the target denies invoke, nor on being served when it does; because the request never terminates, client-side timeouts and retries fire instead of the gateway's own answer, and the per-stage fan-out of one deployment to several Lambda targets — the whole point of stage variables — cannot be used at all.

Fix: the apigatewayv2 HTTP execute-api data-plane request path — the stage/integration resolution step that must substitute ${stageVariables.<name>} from the invoked stage's StageVariables before target lookup, and the AWS_PROXY invoke step's lambda:InvokeFunction resource-policy check, which must always emit a response (500 with body {"message":"Internal Server Error"} when the resolved target grants no invoke) rather than leaving the connection open.

Traces: apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-target-without-invoke-permission-rejected.

## Evidence
Cause 1 — replay verdict:
  [mismatch] apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-target-without-invoke-permission-rejected — Error: Check timed out after 60.0s

Expected (AWS):
  GET <invoke-url of the stage whose ${stageVariables.fn} names a Lambda with NO apigateway invoke grant> -> HTTP 500, body exactly {"message":"Internal Server Error"}, and that Lambda is never invoked (nothing reaches its side channel).
  GET <invoke-url of the sibling stage on the same deployment whose ${stageVariables.fn} names a permitted Lambda> -> HTTP 200 with that function's own body; the recorded invocation's stageVariables equals that stage's variable map and requestContext.stage equals that stage's name.
  After lambda:AddPermission grants apigateway.amazonaws.com invoke on the previously ungranted function, the same stage -> HTTP 200 served by its OWN target.

Actual (emulator):
  No HTTP response on either stage's invoke URL. The request never completes; the check is terminated at the 60 s cap with no status code and no body, so not even the positive control (the permitted stage returning 200) is reached. Status, error code and message of the refusal are therefore all absent rather than 500 / {"message":"Internal Server Error"}.

## Steps to Reproduce
Replay the behavioral suite and re-run apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-target-without-invoke-permission-rejected.

Manually:
1. Create two Lambda functions (fn_ok, fn_no) sharing an execution role.
2. CreateApi(ProtocolType="HTTP"); CreateIntegration(IntegrationType="AWS_PROXY", IntegrationMethod="POST", PayloadFormatVersion="2.0", IntegrationUri="arn:aws:lambda:<region>:<account>:function:${stageVariables.fn}"); CreateRoute(RouteKey="GET /who", Target="integrations/<id>"); CreateDeployment.
3. CreateStage(StageName="granted", DeploymentId=<id>, StageVariables={"fn": fn_ok}) and CreateStage(StageName="ungranted", DeploymentId=<id>, StageVariables={"fn": fn_no}).
4. lambda:AddPermission on fn_ok only: Principal=apigateway.amazonaws.com, Action=lambda:InvokeFunction, SourceArn=arn:aws:execute-api:<region>:<account>:<api-id>/*/*.
5. GET https://<api-id>.execute-api.<region>.amazonaws.com/granted/who — expect 200 with fn_ok's body.
6. GET https://<api-id>.execute-api.<region>.amazonaws.com/ungranted/who — expect 500 with body exactly {"message":"Internal Server Error"}, and confirm fn_no was never invoked (use a side channel such as an SQS queue the handler writes to, with the permitted stage's later request as the ordering barrier).
7. AddPermission the same grant on fn_no and GET the "ungranted" stage again — expect 200 served by fn_no, with the invocation's stageVariables == {"fn": fn_no} and requestContext.stage == "ungranted".

Observed: steps 5-7 never return an HTTP response at all.
