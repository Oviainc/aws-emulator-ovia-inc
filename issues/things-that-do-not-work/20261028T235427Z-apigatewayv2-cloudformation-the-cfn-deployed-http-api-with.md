Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudformation.stage-auto-deploy-republishes-on-a-cfn-route-change
Issue-Key: apigatewayv2__cloudformation.stage-auto-deploy-republishes-on-a-cfn-route-change|_poll_http#12|Timeout|time.sleep(interval_s)
Issue-Commit: 92bcdac01f88c8b4612f5537a2ed608090c91758

# apigatewayv2__cloudformation: the CFN-deployed HTTP API with an AutoDeploy $default stage produces no verdict — the deploy/serve path never settles and the auto-deploy check is killed at the harness cap (1 cause)

> _Note: this issue was identified against the code base at commit `92bcdac01f88c8b4612f5537a2ed608090c91758`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause, grounded in one failing behavioral check. It is NOT covered by the two open issues on this surface: 20261027T192238Z-apigatewayv2-cloudformation-a-stack-whose-aws-apigatewayv2-api.md is keyed to the inline-OpenAPI-`Body` import variant, and 20261026T235900Z-apigatewayv2-cloudformation-2-net-new-root-causes-aws.md to DisableExecuteApiEndpoint; this check declares its API, integration and routes as ordinary AWS::ApiGatewayV2::{Api,Integration,Route} resources with no Body at all, and carries the auto-deploy republish semantics neither issue records.

Cause 1 of 1 — a stack whose AWS::ApiGatewayV2::Api is declared property-by-property (ProtocolType HTTP), with an AWS_PROXY integration onto an inline python3.12 Lambda, a route set controlled by a template Condition, and a $default stage with AutoDeploy true, never reaches the point where anything can be measured: the check is cut off by the harness's 60s cap with no assertion result.
AWS: recorded behavior only (CloudFormation's AWS::ApiGatewayV2::Stage AutoDeploy property and the HTTP API stage docs at https://docs.aws.amazon.com/apigateway/latest/developerguide/http-api-stages.html). CreateStack with {AWS::IAM::Role, AWS::Lambda::Function (inline ZipFile echo handler), AWS::ApiGatewayV2::Api (ProtocolType HTTP), AWS::Lambda::Permission, AWS::ApiGatewayV2::Integration (AWS_PROXY, IntegrationMethod POST, PayloadFormatVersion 2.0, IntegrationUri = the function ARN), AWS::ApiGatewayV2::Route 'GET /v1', AWS::ApiGatewayV2::Stage ($default, AutoDeploy true)} reaches CREATE_COMPLETE and its outputs carry the API id; GET <$default invoke URL>/v1 answers 200 with the proxied Lambda's body (routeKey 'GET /v1'), while GET /v2 answers 404; GetStage($default) reports a non-empty DeploymentId. An UpdateStack that flips one parameter so the template adds 'GET /v2' is then served by the AutoDeploy stage with NO explicit CreateDeployment — GET /v2 becomes 200 with routeKey 'GET /v2', /v1 keeps serving, and GetStage($default).DeploymentId is a NEW id — whereas a twin stage created out of band with AutoDeploy false and an explicit DeploymentId keeps 404ing /v2 (and keeps 200ing /v1) until an explicit CreateDeployment(ApiId, StageName) publishes the post-update route set to it, after which it serves /v2 and reports the new deployment id.
Emulator: no observable outcome at all. The replay verdict is only `Check timed out after 60.0s`, so the emulator supplies neither a terminal stack status nor a served route within the suite's budget — a caller is left hanging rather than given a status, a 4xx or a response it can branch on.
Hypothesis (where it stalls): the check's two unbounded-looking waits before any assertion are (a) `_deploy_cfn`'s DescribeStacks convergence poll, budget 600s, and (b) `_poll_http` on GET <$default>/v1 accepting only 200, budget 180s, 4s interval; both exceed the 60s harness cap, so the stall is in one of them — the stack never reaching a terminal create state, or the execute-api data plane of the CFN-created $default stage never answering 200 for its route. The sibling open issue above records exactly that pair of stall points for the Body-import variant of the same resource family.
Why: the standard way an HTTP API is shipped (CloudFormation-declared API + routes + an AutoDeploy stage, then a route added by a stack update) cannot be emulated, and the failure is indistinguishable from a hung control plane: no CREATE_FAILED/ROLLBACK to react to, no served route, no DeploymentId movement, and no way to tell "unsupported" from "still creating".
Fix: the AWS::ApiGatewayV2::{Api,Integration,Route,Stage} provisioners in the cloudformation crate's resource-provisioner layer must always drive the stack to a terminal state (never leave CreateStack/UpdateStack unresolved), must register the stage's AutoDeploy flag and mint a NEW DeploymentId whenever the api's route set changes, must leave an AutoDeploy=false stage pinned to its explicit DeploymentId, and the apigatewayv2 execute-api data plane must serve a stage's pinned deployment snapshot (200 through the AWS_PROXY integration for routes in that snapshot, 404 for routes added after it).
Traces: apigatewayv2__cloudformation.stage-auto-deploy-republishes-on-a-cfn-route-change

## Evidence
Failing check apigatewayv2__cloudformation.stage-auto-deploy-republishes-on-a-cfn-route-change — verdict [mismatch], detail: `Error: Check timed out after 60.0s` (no assertion reached).

Expected (AWS), in order:
  1. CreateStack(template above, IncludeSecondRoute='false') -> CREATE_COMPLETE, Outputs RouteSetOut='v1', ApiIdOut=<api id>.
  2. GET <$default>/v1 -> 200, body {"routeKey": "GET /v1", "rawPath": "/v1"}; GET <$default>/v2 -> 404.
  3. CreateDeployment(ApiId) + CreateStage(ApiId, StageName='frozen', DeploymentId=<that>, AutoDeploy=False) -> GET <frozen>/v1 -> 200 routeKey 'GET /v1'; GetStage($default).DeploymentId non-empty.
  4. UpdateStack(IncludeSecondRoute='true') -> UPDATE_COMPLETE; GetRoutes == {'GET /v1','GET /v2'}.
  5. GET <$default>/v2 -> 200 routeKey 'GET /v2' with NO CreateDeployment issued for that stage; GET <$default>/v1 still 200; GetStage($default).DeploymentId is a NEW id.
  6. GET <frozen>/v2 stays 404 across the whole sampling window while GET <frozen>/v1 stays 200; GetStage('frozen').DeploymentId unchanged.
  7. CreateDeployment(ApiId, StageName='frozen') -> new id; GET <frozen>/v2 -> 200 routeKey 'GET /v2'; GetStage('frozen').DeploymentId == that new id.

Actual (emulator): none of the above was reached — the check was cut off at the 60s cap while polling. The two waits that can absorb that budget are `_deploy_cfn`'s DescribeStacks poll (600s budget, 5s interval, terminal set {CREATE_COMPLETE, CREATE_FAILED, ROLLBACK_COMPLETE, ROLLBACK_FAILED, GONE}) and `_poll_http` on GET <$default>/v1 accepting only 200 (180s budget, 4s interval).

## Steps to Reproduce
1. CreateStack (Capabilities=[CAPABILITY_IAM, CAPABILITY_NAMED_IAM], Parameters ApiName/FunctionName/RoleName unique, IncludeSecondRoute='false') with: AWS::IAM::Role (lambda.amazonaws.com trust, inline logs policy); AWS::Lambda::Function (python3.12, Code.ZipFile echo handler returning {"routeKey": requestContext.routeKey, "rawPath": rawPath}); AWS::ApiGatewayV2::Api (ProtocolType HTTP); AWS::Lambda::Permission (Principal apigateway.amazonaws.com, SourceArn execute-api ${Api}/*/*); AWS::ApiGatewayV2::Integration (AWS_PROXY, IntegrationMethod POST, PayloadFormatVersion '2.0', IntegrationUri = function ARN); AWS::ApiGatewayV2::Route 'GET /v1'; AWS::ApiGatewayV2::Route 'GET /v2' behind Condition IncludeSecondRoute=='true'; AWS::ApiGatewayV2::Stage ($default, AutoDeploy true); Outputs ApiIdOut=Ref Api, RouteSetOut.
2. Poll DescribeStacks for a terminal create state — AWS reaches CREATE_COMPLETE promptly. Record whether the emulator ever leaves CREATE_IN_PROGRESS.
3. GET <$default stage invoke URL>/v1 — expect 200 with routeKey 'GET /v1'; GET /v2 — expect 404. Record what the emulator serves.
4. CreateDeployment(ApiId) then CreateStage(ApiId, StageName='frozen', DeploymentId=<it>, AutoDeploy=False); GET <frozen>/v1 — expect 200.
5. UpdateStack with IncludeSecondRoute='true' and wait for UPDATE_COMPLETE; GetRoutes must report both route keys.
6. Without issuing any CreateDeployment for $default: GET <$default>/v2 must become 200 with routeKey 'GET /v2', /v1 must stay 200, and GetStage($default).DeploymentId must differ from the pre-update value.
7. GET <frozen>/v2 must stay 404 (sampled at least twice over the window) and GetStage('frozen').DeploymentId must be unchanged, until CreateDeployment(ApiId, StageName='frozen'), after which GET <frozen>/v2 must be 200 and GetStage('frozen').DeploymentId must equal the new deployment id.
