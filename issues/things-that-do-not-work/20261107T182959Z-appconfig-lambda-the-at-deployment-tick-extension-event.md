Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__lambda.at-deployment-tick-directive-rollback-reverts-the-deployment
Issue-Key: appconfig__lambda.at-deployment-tick-directive-rollback-reverts-the-deployment|run#49|AssertionError|assert "PercentageComplete" in tick_event, (
Issue-Commit: c3a6f169c346304d6b4c388bf8378b1c143b9d70

# appconfig__lambda: the AT_DEPLOYMENT_TICK extension event omits PercentageComplete, so a tick handler cannot see the rollout position it is being asked to rule on

> _Note: this issue was identified against the code base at commit `c3a6f169c346304d6b4c388bf8378b1c143b9d70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDeployment

## Description
One net-new root cause (Cause 1 of 1).

Failing check id: appconfig__lambda.at-deployment-tick-directive-rollback-reverts-the-deployment.

PRIOR WORK, read first: open issue 20261106T033908Z-appconfig-lambda-2-net-new-causes-a-pre.md lists this check id, but its cause for it was "AT_DEPLOYMENT_TICK is never invoked at all" and it does not carry this replay's key; the report does not mark this check OWNED or ALREADY FILED. In THIS replay the invocation DOES happen — a tick event reaches the observation queue and the check gets past the arrival barrier and past the assertions on DeploymentNumber and Environment.Id — and then fails on the event's CONTENT. Related open issues that likewise do not cover this content cause: 20261101T210635Z-appconfig-lambda-a-deployment-that-receives-a-rollback.md, 20261105T111951Z-appconfig-lambda-appconfig-extension-action-points-never-invoke.md and 20261027T010737Z-appconfig-lambda-backed-extension-action-points-still-never.md (all about the dispatch not happening).

AWS: the extension event payload is not modelled in Smithy (StartDeployment is POST /applications/{ApplicationId}/environments/{EnvironmentId}/deployments, success 201, output Deployment, errors BadRequestException 400 / ConflictException 409 / ResourceNotFoundException 404 / InternalServerException 500; CreateExtension is POST /extensions, success 201, with an @required Actions map keyed by action point). Recorded/documented behavior only: an extension action bound to AT_DEPLOYMENT_TICK is invoked repeatedly while a deployment is in flight, and the event body it receives reports the rollout POSITION the tick is asking about — the same quantity Smithy models on Deployment as PercentageComplete (target Percentage, range 1.0-100.0, documented "The percentage of targets for which the deployment is available") — alongside the deployment's identity (DeploymentNumber, PreviousDeploymentNumber, Application, Environment, ConfigurationProfile, Content/ContentType/ContentVersion, InvocationId, Parameters, Type). That position is the input a tick handler rules on when it answers {"Directive": "CONTINUE"|"ROLL_BACK", "Description": "<text>"}.

Emulator: the tick event the emulator posts to the action target carries only the keys ['Application', 'ConfigurationProfile', 'Content', 'ContentType', 'ContentVersion', 'DeploymentNumber', 'Environment', 'InvocationId', 'Parameters', 'PreviousDeploymentNumber', 'Type'] — PercentageComplete is absent, so the check fails with "an AT_DEPLOYMENT_TICK event must report the rollout position it is asking about". Everything up to that point now matches: the Lambda is invoked, the event names the deployment under tick and the environment under deployment.

Why: AT_DEPLOYMENT_TICK plus a ROLL_BACK directive is AppConfig's programmable abort, and the rollout position is the whole basis for the decision — a handler aborts when the blast radius is still small, or lets a release continue once it is nearly done. A handler written against AWS that reads event["PercentageComplete"] raises KeyError against this emulator (or, if it uses .get, silently reads None and takes the wrong branch), so the directive it returns is not the one it would return on AWS: every percentage-gated gate, canary rule and progressive-abort policy is unimplementable here even though the invocation now arrives.

Fix: the AppConfig extension action-point dispatch seam where the AT_DEPLOYMENT_TICK event body is built (driven from the deployment state machine's growth-tick loop) — the event must carry the deploying deployment's current rollout percentage under the member name AWS uses on Deployment, PercentageComplete, as of the tick being dispatched, in addition to the identity members it already carries.

Traces: appconfig__lambda.at-deployment-tick-directive-rollback-reverts-the-deployment. Note for the fixer: the legs after this assertion are still ungraded in this replay — the ROLL_BACK directive terminating the deployment ROLLED_BACK, the EventLog carrying ROLLBACK_STARTED and ROLLBACK_COMPLETED and not DEPLOYMENT_COMPLETED, the directive's Description appearing in an EventLog Description, ROLLBACK_STARTED entries attributed TriggeredBy 'EXTENSION', version 1's bytes back in service, StopDeployment on the terminal deployment refused BadRequestException with 'Deployment <n> for App <app-id> and Environment <env-id> cannot be stopped, it has a status of <State>.', and the identical deployment completing once the directive is CONTINUE again. Those belong to the issues named under PRIOR WORK; this issue is closed by the event body carrying PercentageComplete, and re-replaying the check is how a fixer discovers what the next barrier is. Generated function/extension names, ids, deployment numbers and configuration bytes in that run are per-run values that only fill the templates.

## Evidence
Behavioral replay of suite simparity-behavioral-aws-v1 against the booted emulator: 205/240 passed (85.4%), 28 mismatched, 7 errored, 0 skipped; bootstrap_signal false. Per-service appconfig__lambda 1/4 (25%), 3 failed — the other two failures are marked OWNED by open issues and are excluded here.

[mismatch] appconfig__lambda.at-deployment-tick-directive-rollback-reverts-the-deployment:
  File "check_appconfig__lambda_at_deployment_tick_directive_rollback_reverts_the_deployment.py", line 442, in run
    assert "PercentageComplete" in tick_event, (
  AssertionError: an AT_DEPLOYMENT_TICK event must report the rollout position it is asking about; keys were ['Application', 'ConfigurationProfile', 'Content', 'ContentType', 'ContentVersion', 'DeploymentNumber', 'Environment', 'InvocationId', 'Parameters', 'PreviousDeploymentNumber', 'Type']

The check wires an SQS queue as the only way to observe the event: its Lambda forwards each event it receives to the queue and returns {"Directive": os.environ["DIRECTIVE"], "Description": os.environ["REASON"]}. It uses DeploymentDurationInMinutes=2 on purpose ("A 0-minute deployment invokes no action point at all"). The assertions immediately before the failing one PASSED, which is what makes this cause net-new rather than the dispatch gap filed earlier:
  - a tick event arrived at all ("the AT_DEPLOYMENT_TICK action point never invoked the Lambda — nothing reached the observation queue" did NOT fire),
  - tick_event["DeploymentNumber"] == the deployment's number,
  - tick_event["Environment"]["Id"] == the environment id.
Expected (AWS): the event body additionally reports the rollout position, i.e. a PercentageComplete member giving the deploying deployment's current percentage at that tick.
Actual (emulator): no such key; the 11 keys listed above are everything the event carries.

Smithy consulted (services/service-com-amazonaws-appconfig-amazonappconfig/smithy-model/com-amazonaws-appconfig-amazonappconfig.json): StartDeployment — POST /applications/{ApplicationId}/environments/{EnvironmentId}/deployments, code 201, output Deployment, errors BadRequestException 400 / ConflictException 409 / ResourceNotFoundException 404 / InternalServerException 500; Deployment$PercentageComplete — target Percentage (range 1.0-100.0), doc "The percentage of targets for which the deployment is available"; DeploymentState enum = [BAKING, VALIDATING, DEPLOYING, COMPLETE, ROLLING_BACK, ROLLED_BACK, REVERTED]; CreateExtension — POST /extensions, code 201, @required Actions map. The extension event payload itself is not modelled: recorded/documented behavior, and the member name is the one the API uses for that quantity.

Both constituent services are stood up in this checkout (appconfig, lambda), so this is a fidelity finding, not a bootstrap gap.

## Steps to Reproduce
Boot the emulator and point appconfig, appconfigdata, lambda, iam and sqs clients at its data listener.
1. CreateQueue as the observation sink; create an execution role allowing sqs:SendMessage on it; create a python Lambda whose handler sends the whole event to that queue (json.dumps(event, default=str)) and returns {"Directive": os.environ["DIRECTIVE"], "Description": os.environ["REASON"]}.
2. AddPermission for Principal appconfig.amazonaws.com on the function, and create a role appconfig.amazonaws.com can assume that allows lambda:InvokeFunction on the function ARN.
3. CreateExtension(Actions={'AT_DEPLOYMENT_TICK': [{'Name':'tick','Uri':<fn-arn>,'RoleArn':<invoke-role>}]}).
4. Create an application, an environment and a hosted configuration profile; CreateHostedConfigurationVersion twice with distinguishable bytes (versions 1 and 2).
5. CreateExtensionAssociation binding the extension to arn:aws:appconfig:<region>:<account>:application/<app-id>/environment/<env-id>.
6. CreateDeploymentStrategy with DeploymentDurationInMinutes=2 (NOT 0 — a zero-duration deployment invokes no action point), GrowthFactor=100, FinalBakeTimeInMinutes=0, ReplicateTo='NONE'.
7. With DIRECTIVE='CONTINUE', StartDeployment of version 1 and ReceiveMessage from the queue until a tick event arrives.
   EXPECTED (AWS): the event body carries a PercentageComplete member reporting the deployment's current rollout position at that tick, alongside DeploymentNumber, PreviousDeploymentNumber, Application, Environment, ConfigurationProfile, Content/ContentType/ContentVersion, InvocationId, Parameters and Type.
   OBSERVED: the body has exactly ['Application', 'ConfigurationProfile', 'Content', 'ContentType', 'ContentVersion', 'DeploymentNumber', 'Environment', 'InvocationId', 'Parameters', 'PreviousDeploymentNumber', 'Type'] — no PercentageComplete.
8. Collect several consecutive tick events of the same deployment: EXPECTED their PercentageComplete values to be non-decreasing and to track what GetDeployment reports for that deployment at the same moments.
Re-replay appconfig__lambda.at-deployment-tick-directive-rollback-reverts-the-deployment to confirm this cause is closed; it drives a 2-minute deployment, so it needs a wall-clock budget larger than the 180 s per-check cap (the check declares _TICK_BUDGET_S=900 and _DEPLOY_TIMEOUT_S=300). Expect the check to then advance to the ROLL_BACK legs tracked by the issues named in the description.
