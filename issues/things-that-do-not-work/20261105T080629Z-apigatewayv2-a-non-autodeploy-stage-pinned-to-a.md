Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.deep-apigatewayv2-3-stagesem-manual-stage-serves-deployment-snapshot
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.deep-apigatewayv2-3-stagesem-manual-stage-serves-deployment-snapshot|_wait_stable#24|Timeout|status, _, text = _http("GET", url, headers=headers)
Issue-Commit: 4fb69df4492bb482aa671e07d0cac65e9605f0df

# apigatewayv2: a non-autoDeploy stage pinned to a DeploymentId serves nothing, so the deployment-snapshot contract reaches no verdict (1 cause)

> _Note: this issue was identified against the code base at commit `4fb69df4492bb482aa671e07d0cac65e9605f0df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
One net-new root cause in the apigatewayv2 emulator, grounded in the single failing behavioral check in this replay that is neither marked 'ALREADY FILED' (blocked on an unimplemented scaffold stub) nor 'OWNED by' an open issue.

Related open issues describe neighbouring symptoms but do not carry this replay's key, so none of them covers it: 20261101T041930Z-apigatewayv2-3-net-new-root-causes-the-http.md names the same behaviour (its Cause 2 of 3) but its stored replay key for this check pins a different failure frame (`_http#24|Timeout|with urllib.request.urlopen(req, timeout=timeout_s) as resp:`), so the suite did not attribute this replay to it; 20261024T120951Z-apigatewayv2-the-execute-api-data-plane-never-serves.md, 20261027T053233Z-apigatewayv2-an-http-api-aws-proxy-route-still.md and 20261029T194243Z-apigatewayv2-2-net-new-root-causes-an-http.md describe the data plane generally without stating the stage/deployment-snapshot rule. This issue must be closed on its own replay key.

=== Cause 1 of 1: a stage created with an explicit DeploymentId and no AutoDeploy does not serve its deployment snapshot — the manual/autoDeploy stage split produces no verdict ===
Failing check: apigatewayv2.deep-apigatewayv2-3-stagesem-manual-stage-serves-deployment-snapshot

AWS: Stage semantics for HTTP APIs (https://docs.aws.amazon.com/apigateway/latest/developerguide/http-api-stages.html). A stage created with an explicit `DeploymentId` and without `AutoDeploy` serves the frozen configuration snapshot captured by that deployment. A later API edit (here `UpdateIntegration` repointing `IntegrationUri` from function A to function B) must NOT change what that stage serves, and `GetStage.deploymentId` for it must stay the deployment it was bound to until an explicit `CreateDeployment`. A sibling `$default` stage with `AutoDeploy=true` must pick the edit up by itself and must report a NEW `deploymentId` afterwards. `CreateDeployment(ApiId, StageName=<pinned stage>)` must mint a deployment id different from the first, rebind that stage (`GetStage.deploymentId` == the new id) and make it serve the new configuration. Both stages must answer the route (`GET /snap`) on their stage invoke URL with the integration's response (HTTP 200 and the function's body). Smithy (service com.amazonaws.apigatewayv2#ApiGatewayV2): CreateStage input carries `DeploymentId` (Id, jsonName deploymentId) and `AutoDeploy` (__boolean, jsonName autoDeploy), POST /v2/apis/{ApiId}/stages, success 201; CreateDeployment input {ApiId (httpLabel, required), Description, StageName (StringWithLengthBetween1And128)} -> output {DeploymentId (Id), AutoDeployed, CreatedDate, DeploymentStatus, DeploymentStatusMessage, Description}, POST /v2/apis/{ApiId}/deployments, success 201, errors BadRequestException(400)/ConflictException(409)/NotFoundException(404)/TooManyRequestsException(429); GetStage -> GET /v2/apis/{ApiId}/stages/{StageName}, success 200, output carries `autoDeploy` and `deploymentId`. The snapshot/pinning rule itself is not expressible in the model — it is documented stage behaviour (AWS doc / recorded behavior only).
Emulator: The check creates two Lambda functions A and B (each returning its MARKER env value as the body), one AWS_PROXY integration pointing at A, route `GET /snap`, an `$default` stage with AutoDeploy=true, an explicit CreateDeployment (D1) and a `pinned` stage bound to D1, then waits for BOTH stage invoke URLs to serve (200, markerA) continuously before asserting anything. That readiness gate never passes: the replay produces no answer at all from either stage URL and is cut off by the suite's 60 s cap (verdict: `Check timed out after 60.0s`). Consequently none of the snapshot contract is observable — no hold of the snapshot across UpdateIntegration, no unmoved `deploymentId`, no release on explicit CreateDeployment.
Why: Deployment pinning is the mechanism clients use to hold production on a known-good snapshot while they edit an API. A client doing blue/green or canary releases loses its isolation entirely: either an in-flight edit is exposed on a stage that was meant to be frozen, or (as here) the stage serves nothing and the release never completes, and `GetStage.deploymentId` stops being a trustworthy description of what a stage serves.
Fix: the stage/deployment state model (bind a stage to a snapshot of routes+integrations rather than to live configuration; mint a new deployment id both on CreateDeployment and on autoDeploy; rebind the named stage when CreateDeployment carries StageName), plus the execute-api data-plane stage-resolution step, which must dispatch from the stage's bound snapshot rather than current configuration and must answer the route at the stage invoke URL.
Traces: apigatewayv2.deep-apigatewayv2-3-stagesem-manual-stage-serves-deployment-snapshot

## Evidence
Suite: simparity-behavioral-aws-v1, 102/242 passed (42.1%); apigatewayv2 90/167 (54%), 77 failed. This is the only failing check in the whole report that is neither 'ALREADY FILED' nor 'OWNED by' an open issue.

[mismatch] apigatewayv2.deep-apigatewayv2-3-stagesem-manual-stage-serves-deployment-snapshot
  Error: Check timed out after 60.0s
  Expected (AWS): both the AutoDeploy `$default` stage and the `pinned` stage (created with DeploymentId=D1) answer GET <stage-invoke-url>/snap with (200, markerA); GetStage(pinned).autoDeploy is not true and GetStage(pinned).deploymentId == D1; after UpdateIntegration repoints IntegrationUri to function B, `$default` converges to (200, markerB) with a changed deploymentId while every sample of the pinned stage stays (200, markerA) for the whole hold window (3x the measured autoDeploy latency, floored at 15 s) and GetStage(pinned).deploymentId is still D1; CreateDeployment(ApiId, StageName='pinned') returns D2 != D1, the pinned stage then serves (200, markerB), and GetStage(pinned).deploymentId == D2. (markerA/markerB, the api id and the deployment ids are per-run values that fill this template, not requirements.)
  Actual (emulator): the pre-assertion stability gate — both stage invoke URLs serving (200, markerA) continuously — never passes; no response is ever accepted from either stage URL and the run is cut at the 60 s suite cap. No snapshot hold and no snapshot release is observable, and the pinned-stage deploymentId contract is never exercised.

## Steps to Reproduce
Clone the repo, build and boot the emulator, then replay the behavioral suite (run_behavioral_suite) and look at check id apigatewayv2.deep-apigatewayv2-3-stagesem-manual-stage-serves-deployment-snapshot.

1. Create two Lambda functions A and B whose handler returns `os.environ["MARKER"]` as the body with statusCode 200 (distinct MARKER values).
2. CreateApi(ProtocolType='HTTP'); CreateIntegration(IntegrationType='AWS_PROXY', IntegrationUri=<arn of A>, IntegrationMethod='POST', PayloadFormatVersion='2.0'); CreateRoute(RouteKey='GET /snap', Target='integrations/<id>'); add a lambda:InvokeFunction permission for execute-api on both functions.
3. CreateStage(StageName='$default', AutoDeploy=true); CreateDeployment(ApiId) -> D1; CreateStage(StageName='pinned', DeploymentId=D1).
4. GET <invoke-url>/snap on both the `$default` and the `pinned` stage URL — expect (200, markerA) from each, repeatedly. Observed: neither URL ever answers, so the check never gets past this gate (timeout at the 60 s cap).
5. UpdateIntegration(ApiId, IntegrationId, IntegrationUri=<arn of B>) — expect `$default` to converge to (200, markerB) with a new GetStage('$default').deploymentId, while every sample of the pinned stage stays (200, markerA) and GetStage('pinned').deploymentId == D1.
6. CreateDeployment(ApiId, StageName='pinned') -> D2 != D1 — expect the pinned stage to serve (200, markerB) and GetStage('pinned').deploymentId == D2.
