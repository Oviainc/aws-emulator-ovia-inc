Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudformation.stage-auto-deploy-republishes-on-a-cfn-route-change
Issue-Key: apigatewayv2__cloudformation.stage-auto-deploy-republishes-on-a-cfn-route-change|_poll_http#12|TimeoutError|raise TimeoutError(f"{description} ({url}): the run budget ended first; not graded")
Issue-Commit: cc767c9c860d2029dd9667f4e637f1644e38e746

# apigatewayv2__cloudformation: a CFN-managed AutoDeploy $default stage never serves, so a route added by a stack update is never republished

> _Note: this issue was identified against the code base at commit `cc767c9c860d2029dd9667f4e637f1644e38e746`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStage

## Description
One net-new root cause.

Cause 1 of 1: an AWS::ApiGatewayV2::Stage with AutoDeploy on a CFN-deployed HTTP API never serves its routes, so the auto-republish-on-route-change behaviour is unobservable.

AWS:      Smithy: Stage.AutoDeploy (boolean) — when true, API Gateway automatically redeploys the API to that stage whenever the API's routes or integrations change, with no CreateDeployment call. So a CloudFormation stack that declares an HTTP API, a route and a stage with AutoDeploy=true serves that route on the stage's invoke URL immediately after the stack reaches CREATE_COMPLETE; after a stack UPDATE that adds or changes a route, the AutoDeploy stage serves the new route (the integration's status, 200 here), while a stage pinned to an explicit DeploymentId keeps serving the OLD snapshot and answers 404 for the new route. recorded behavior only for the data-plane statuses: the 200 from the new route on the AutoDeploy stage and the 404 from the pinned stage are the recorded responses; the api id, stage name and route path are values from the template and the request.
Emulator: the AutoDeploy $default stage created by the stack never answers at all. Polling the route exhausts the run budget with no status: TimeoutError: "GET /v1 on the AutoDeploy $default stage created by the stack (https://<host>/_aws/execute-api/<apiId>/$default/v1): the run budget ended first; not graded". Because the baseline leg never serves, the republish-after-update leg and the pinned-stage 404 leg are never reached either.
Why:      A client deploying an HTTP API through CloudFormation with AutoDeploy gets a stack that reports success but an endpoint that answers nothing. It cannot tell a propagation delay from a permanently dead endpoint, and the central reason to set AutoDeploy — that a template change to a route takes effect without an explicit deployment — can never be relied upon, while a pinned stage's isolation from that change is equally unverifiable.
Fix:      The execute-api data plane for CFN-materialised apigatewayv2 stages, and the AutoDeploy republish path in the Stage/Route state: a stage with AutoDeploy=true must bind the API's current route set and serve it, and re-bind it on every route/integration mutation, including those applied by a stack update.
Traces:   apigatewayv2__cloudformation.stage-auto-deploy-republishes-on-a-cfn-route-change. The open issue 20261028T235427Z-apigatewayv2-cloudformation-the-cfn-deployed-http-api-with.md describes a similar non-settling deploy/serve path but does not carry this check's replay key, so it does not cover this cause; close them together if they share one fix.

## Evidence
Expected: after the stack reaches CREATE_COMPLETE, GET /v1 on the AutoDeploy $default stage returns the integration's 200; after the stack update that changes the route, the same stage serves the new route with 200 while the stage pinned to an explicit DeploymentId answers 404 for it over the sampling window. Actual: no status is ever returned from the AutoDeploy stage — `TimeoutError: GET /v1 on the AutoDeploy $default stage created by the stack (https://127.0.0.1:<port>/_aws/execute-api/<apiId>/$default/v1): the run budget ended first; not graded` (check_apigatewayv2__cloudformation_stage_auto_deploy_republishes_on_a_cfn_route_change.py line 79, via _poll_http at line 322). Host, port and api id are per-run values; the invariant is that no response is produced.

## Steps to Reproduce
Deploy a CloudFormation stack declaring an AWS::ApiGatewayV2::Api (HTTP), a route, and an AWS::ApiGatewayV2::Stage named $default with AutoDeploy=true plus a second stage pinned to an explicit DeploymentId. Wait for CREATE_COMPLETE, then GET /v1 on the AutoDeploy stage's invoke URL. Then update the stack to change the route and GET again on both stages. Replay check apigatewayv2__cloudformation.stage-auto-deploy-republishes-on-a-cfn-route-change.
