Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.lambda-deployment-stop-rolls-back-alias-and-refuses-restop
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.lambda-deployment-stop-rolls-back-alias-and-refuses-restop|run#14|AssertionError|assert stop_resp.get("statusMessage") == _STOP_STATUS_MESSAGE, (
Issue-Commit: 5772abe834f24ca0a7d173ddfaf967982b48def5

# CodeDeploy StopDeployment returns a rollback-eligibility sentence as statusMessage instead of AWS's stop-accepted message (1 cause)

> _Note: this issue was identified against the code base at commit `5772abe834f24ca0a7d173ddfaf967982b48def5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopDeployment

## Description
Cause 1 of 1: StopDeployment's `statusMessage` carries emulator-authored rollback-eligibility prose instead of AWS's accepted-stop message.

AWS: Smithy (codedeploy-2014-10-06) models StopDeploymentOutput as {status: StopStatus, statusMessage: Message}, documenting statusMessage only as "An accompanying status message." — the exact wording is recorded behavior only. On a successful StopDeployment of a non-terminal deployment (here a Lambda TimeBasedCanary deployment with autoRollbackConfiguration {enabled:true, events:[DEPLOYMENT_STOP_ON_REQUEST]}, stopped with autoRollbackEnabled=true), real AWS answers HTTP 200 with status 'Succeeded' and the fixed statusMessage text `No more commands will be scheduled for execution in the deployment instances`. That message is entirely fixed text: it interpolates nothing from the request or from service state — no deployment id, no group name, no version.

Emulator: status is 'Succeeded' (correct), but statusMessage is the rollback-eligibility sentence `Deployment <deployment-id> cannot be rolled back: no revision was deployed to this deployment group before it` (observed as `Deployment d-SXPTMCEWD cannot be rolled back: no revision was deployed to this deployment group before it`; `d-SXPTMCEWD` is the per-run deployment id the request carried, not part of any requirement). So the emulator (a) substitutes a different message class for the accept path, and (b) interpolates a variable where AWS's text has none.

Why: StopDeployment's statusMessage is the only field that tells a caller what the accepted stop did. A client (or an operator's log/audit scraper) that matches AWS's accepted-stop sentence sees instead a sentence asserting the deployment cannot be rolled back — the opposite of the rollback this call requested and performed, so tooling reads a successful auto-rollback stop as a stop that rolled nothing back. It also leaks a decision the response should not be reporting here.

Fix: the StopDeployment handler's response construction in the codedeploy service crate — the branch that fills StopDeploymentOutput.statusMessage after accepting the stop must emit the fixed accepted-stop text rather than reusing the rollback-eligibility message builder. The rollback-eligibility wording belongs on the rollback path (and on the deployment's own errorInformation/rollbackInfo), not on the StopDeployment accept response.

Traces: failing behavioral check codedeploy.lambda-deployment-stop-rolls-back-alias-and-refuses-restop, which asserts stop_resp['statusMessage'] immediately after a 200 StopDeployment (status 'Succeeded' already matched). Note: this check also later exercises the Stopped/MANUAL_STOP settle, alias restoration, stop idempotency and the DeploymentAlreadyCompletedException refusal; those assertions are downstream of this one and are covered by the open issues 20261107T224322Z-codedeploy-lambda-deployments-reach-succeeded-without-shifting-the.md and 20261029T180825Z-codedeploy-deployments-settle-to-succeeded-instantly-no-inprogress.md — neither of those names the statusMessage divergence filed here.

## Evidence
Check: codedeploy.lambda-deployment-stop-rolls-back-alias-and-refuses-restop
Assertion (check_codedeploy_lambda_deployment_stop_rolls_back_alias_and_refuses_restop.py:86):
  assert stop_resp.get("statusMessage") == _STOP_STATUS_MESSAGE

Expected (real AWS, HTTP 200):
  status        = "Succeeded"
  statusMessage = "No more commands will be scheduled for execution in the deployment instances"   (fixed text, no interpolation)

Actual (emulator, HTTP 200):
  status        = "Succeeded"
  statusMessage = "Deployment d-SXPTMCEWD cannot be rolled back: no revision was deployed to this deployment group before it"
                  (template: "Deployment <deploymentId> cannot be rolled back: no revision was deployed to this deployment group before it"; d-SXPTMCEWD is this run's deployment id)

AssertionError: expected statusMessage 'No more commands will be scheduled for execution in the deployment instances', got 'Deployment d-SXPTMCEWD cannot be rolled back: no revision was deployed to this deployment group before it'

## Steps to Reproduce
1. Create a versioned Lambda function with two versions (v1, v2) and an alias pointing at v1.
2. CreateApplication with computePlatform=Lambda.
3. CreateDeploymentConfig with computePlatform=Lambda and trafficRoutingConfig {type: TimeBasedCanary, timeBasedCanary: {canaryPercentage: 10, canaryInterval: 5}} so the deployment stays non-terminal.
4. CreateDeploymentGroup on that application using that deployment config.
5. CreateDeployment with the Lambda AppSpec revision (alias v1 -> v2) and autoRollbackConfiguration {enabled: true, events: ["DEPLOYMENT_STOP_ON_REQUEST"]}.
6. Poll GetDeployment until status is non-terminal (InProgress/Baking/Ready).
7. Call StopDeployment(deploymentId=<id>, autoRollbackEnabled=true).
8. Inspect the response: status is 'Succeeded' as AWS returns, but statusMessage is 'Deployment <id> cannot be rolled back: no revision was deployed to this deployment group before it' instead of AWS's 'No more commands will be scheduled for execution in the deployment instances'.
