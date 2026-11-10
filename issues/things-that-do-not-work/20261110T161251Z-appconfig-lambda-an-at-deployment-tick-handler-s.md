Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__lambda.at-deployment-tick-directive-rollback-reverts-the-deployment
Issue-Key: appconfig__lambda.at-deployment-tick-directive-rollback-reverts-the-deployment|_drain_one#45|Timeout|msgs = sqs.receive_message(
Issue-Commit: 6cc05b66b5dc4f3acc804c4a889717eb31d245db

# appconfig__lambda: an AT_DEPLOYMENT_TICK handler's ROLL_BACK directive produces no verdict — the tick/rollback scenario never observes a tick event or a reverted deployment

> _Note: this issue was identified against the code base at commit `6cc05b66b5dc4f3acc804c4a889717eb31d245db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDeployment

## Description
Cause 1 of 1: the AT_DEPLOYMENT_TICK extension action point never yields an observable tick-driven rollback. A deployment whose associated extension returns Directive=ROLL_BACK at a growth tick never reverts, and the tick event itself is never forwarded by the handler, so the scenario reaches no verdict before the harness deadline (60s) expires.
AWS: an extension associated to a configuration profile/environment at AT_DEPLOYMENT_TICK is invoked at each growth tick of a non-instant deployment; the handler's returned Directive=ROLL_BACK makes AppConfig roll the deployment back, and GetDeployment then reports a terminal State of ROLLED_BACK (Smithy appconfig DeploymentState enum: BAKING, VALIDATING, DEPLOYING, COMPLETE, ROLLING_BACK, ROLLED_BACK, REVERTED) with a DeploymentEvent whose TriggeredBy is EXTENSION (TriggeredBy enum: USER, APPCONFIG, CLOUDWATCH_ALARM, INTERNAL_ERROR per the model's documentation of the entity that triggered the event); the data plane serves the previous configuration again. Each invocation runs the handler's deployment package, so a handler that forwards its event to SQS makes the tick event observable.
Emulator: the scenario makes no observable progress and ends at the harness ceiling of 60s with no tick event delivered and no rollback. Open issues record the neighbouring causes without carrying this check's replay key: 20261101T210635Z-appconfig-lambda-a-deployment-that-receives-a-rollback.md (tick rollback never reverts), 20261106T033908Z-appconfig-lambda-2-net-new-causes-a-pre.md and 20261027T010737Z-appconfig-lambda-backed-extension-action-points-still-never.md (action points never fire), 20261107T182959Z-appconfig-lambda-the-at-deployment-tick-extension-event.md (tick event omits PercentageComplete). Note for the fixer: part of this check's wall-clock comes from its own 2-minute deployment duration, so re-replay it with the deployment lifecycle (see the appconfig rollout/bake issue filed in this sweep) also fixed, and confirm the terminal state and TriggeredBy rather than only that it finished.
Why: a client that installs a tick extension as a kill switch — inspecting each tick and answering ROLL_BACK when its own health signal is bad — gets a deployment that completes regardless, with no rollback and no EXTENSION-attributed event, so the kill switch is silently inert.
Fix: the AppConfig deployment lifecycle's extension action-point dispatch (invoking the associated Lambda at each growth tick and honouring the returned Directive) and the rollback transition it must drive, which must settle the deployment ROLLED_BACK with TriggeredBy=EXTENSION.
Traces: appconfig__lambda.at-deployment-tick-directive-rollback-reverts-the-deployment.

## Evidence
Expected: with a 2-minute deployment and an extension associated at AT_DEPLOYMENT_TICK whose handler returns {"Directive": "ROLL_BACK", "Description": <reason>}, the handler's forwarded tick events arrive on the observing SQS queue and GetDeployment settles at State=ROLLED_BACK with a DeploymentEvent whose TriggeredBy is EXTENSION; a second StopDeployment on the terminal deployment is then refused BadRequestException (400).
Actual: 'Check timed out after 60.0s' — no tick event was ever forwarded and the deployment never reached a rollback state.

## Steps to Reproduce
1. Create a Lambda that forwards its invocation event to an SQS queue and returns {"Directive": os.environ["DIRECTIVE"], "Description": os.environ["REASON"]}.
2. CreateExtension with an AT_DEPLOYMENT_TICK action whose Uri is that function, and CreateExtensionAssociation to the AppConfig environment/profile.
3. CreateDeploymentStrategy with DeploymentDurationInMinutes=2 (a 0-minute strategy invokes no tick at all) and StartDeployment with DIRECTIVE=ROLL_BACK.
4. Receive from the queue: expect at least one AT_DEPLOYMENT_TICK event; poll GetDeployment: expect a terminal State=ROLLED_BACK with an EventLog entry whose TriggeredBy is EXTENSION.
5. Observe no tick event is delivered and the deployment never rolls back.
