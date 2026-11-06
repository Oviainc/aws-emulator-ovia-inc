Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__lambda.extension-action-point-invokes-lambda-during-deployment
Issue-Key: appconfig__lambda.extension-action-point-invokes-lambda-during-deployment|run#54|KeyError|assert event["TriggeredBy"] == "USER", (
Issue-Commit: ef4505bfafb91e46a48884457977c8eb48b81782

# appconfig__lambda: the PRE_START_DEPLOYMENT extension action-point event omits TriggeredBy, so a customer-issued StartDeployment is not attributed to 'USER'

> _Note: this issue was identified against the code base at commit `ef4505bfafb91e46a48884457977c8eb48b81782`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDeployment

## Description
One net-new root cause. NOTE ON PRIOR WORK: the open issue 20261106T033908Z-appconfig-lambda-2-net-new-causes-a-pre.md (Cause 1) and 20261105T223150Z-appconfig-lambda-a-pre-start-deployment-extension-action.md both describe the earlier, coarser symptom — the action point was never invoked at all and this check was killed at the harness cap. In THIS replay the check was NOT marked OWNED and NOT marked ALREADY FILED: the dispatch now happens and the check advances through six consecutive assertions on the delivered event before failing on a missing member, which is a different, narrower defect than the issues above describe. File/close this one on the event payload itself.

Cause 1 of 1: the event body AppConfig hands a PRE_START_DEPLOYMENT extension action carries no TriggeredBy member.
Failing check id: appconfig__lambda.extension-action-point-invokes-lambda-during-deployment.
AWS: Smithy (services/service-com-amazonaws-appconfig-amazonappconfig/smithy-model/com-amazonaws-appconfig-amazonappconfig.json) models StartDeployment as POST /applications/{ApplicationId}/environments/{EnvironmentId}/deployments, success 201, errors BadRequestException 400 / ConflictException 409 / ResourceNotFoundException 404 / InternalServerException 500, and CreateExtension as POST /extensions (success 201) with an @required Actions map keyed by action point; CreateExtensionAssociation is POST /extensionassociations (success 201). The action-point EVENT body is not modelled — recorded behavior only: when an extension action bound to PRE_START_DEPLOYMENT is invoked by a StartDeployment the customer issued, the event AppConfig passes to the target Lambda carries, besides Application/Environment/ConfigurationProfile (objects with Id), Type ('PreStartDeployment', PascalCase, not the SCREAMING_SNAKE action-point key), ContentVersion (the request's ConfigurationVersion, as a string) and Content (base64 of the exact candidate configuration bytes), a TriggeredBy member whose value is the fixed token 'USER' for a deployment a customer started through the API. TriggeredBy is an enumerated token, not a per-run value: 'USER' for a caller-issued StartDeployment, and (per the AT_DEPLOYMENT_TICK family) 'EXTENSION' for an action-point-requested transition — no part of it comes from the request or from state.
Emulator: the invocation and most of the event are now correct — the check's assertions on event['Application']['Id'], event['Environment']['Id'], event['Type'] == 'PreStartDeployment', event['ContentVersion'] == str(version), base64decode(event['Content']) == the candidate bytes and event['ConfigurationProfile']['Id'] all pass — but the key TriggeredBy is absent from the delivered JSON, so reading it raises KeyError: 'TriggeredBy'.
Why: TriggeredBy is how an extension target tells apart a human/CI-initiated release from one AppConfig itself drove (an extension directive, a rollback). A gate written against AWS either crashes on the missing key (as here) or silently treats every invocation as untriggered, so approval logic that must only veto user-initiated releases cannot be written or exercised at all.
Fix: the AppConfig extension action-point dispatch seam's event serializer in the StartDeployment handler — the event body it builds must include TriggeredBy alongside Application/Environment/ConfigurationProfile/Type/ContentVersion/Content, set to 'USER' for a caller-issued StartDeployment.
Traces: appconfig__lambda.extension-action-point-invokes-lambda-during-deployment (the generated application/environment/profile ids, function and extension names and the Lambda request id in that run are per-run values that only fill the templates).

## Evidence
Behavioral replay of suite simparity-behavioral-aws-v1 against the booted emulator: 204/240 passed (85.0%), 29 mismatched, 7 errored, 0 skipped; bootstrap_signal false (the emulator answered throughout). Per-service appconfig__lambda 1/4 (25%), 3 failed; the other two appconfig__lambda failures are marked OWNED by open issues and are excluded here.

[error] appconfig__lambda.extension-action-point-invokes-lambda-during-deployment
  Error: File "run.py", line 353, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
  File "check_appconfig__lambda_extension_action_point_invokes_lambda_during_deployment.py", line 442, in run
    assert event["TriggeredBy"] == "USER", (
  KeyError: 'TriggeredBy'

The failing assertion reads: `assert event["TriggeredBy"] == "USER", (f"a StartDeployment call the customer made reports TriggeredBy 'USER', got {event['TriggeredBy']!r}")`. The event under test is the one the extension's Lambda target forwarded verbatim to an SQS observation queue (`_ACTION_SRC` sends json.dumps(event) to QUEUE_URL), so what is asserted on is exactly the body the emulator delivered.

Expected (AWS): the drained PRE_START_DEPLOYMENT event contains TriggeredBy == 'USER'.
Actual (emulator): the event has no TriggeredBy key at all (KeyError). Every earlier assertion on the same event passed, so the divergence is isolated to this one member:
  - event['Application']['Id'] == the request's ApplicationId — PASS
  - event['Environment']['Id'] == the request's EnvironmentId — PASS
  - event['Type'] == 'PreStartDeployment' — PASS
  - event['ContentVersion'] == str(version) — PASS
  - base64.b64decode(event['Content']) == the exact candidate bytes — PASS
  - event['ConfigurationProfile']['Id'] == the request's ConfigurationProfileId — PASS
  - event['TriggeredBy'] == 'USER' — KeyError

Smithy consulted: StartDeployment POST /applications/{ApplicationId}/environments/{EnvironmentId}/deployments, code 201, errors BadRequestException/400, ConflictException/409, ResourceNotFoundException/404, InternalServerException/500; CreateExtension POST /extensions, code 201, @required Actions (ActionsMap, 1..5); CreateExtensionAssociation POST /extensionassociations, code 201. The action-point event payload is not modelled, so the TriggeredBy requirement is recorded behavior.

Both constituent services are bootstrapped in this checkout (appconfig, lambda).

## Steps to Reproduce
Boot the emulator and point appconfig, appconfigdata, lambda, iam and sqs clients at its data listener.
1. Create an SQS queue as the observation sink, plus an execution role allowing sqs:SendMessage on it; create a python Lambda whose handler forwards its whole event to that queue (json.dumps(event)) and returns {}.
2. AddPermission for Principal appconfig.amazonaws.com on the function, and create a role appconfig.amazonaws.com can assume that allows lambda:InvokeFunction on the function ARN.
3. CreateExtension(Name=<ext>, Actions={'PRE_START_DEPLOYMENT': [{'Name': 'gate', 'Uri': <fn-arn>, 'RoleArn': <invoke-role-arn>}]}).
4. Create an AppConfig application, an environment, and a hosted (LocationUri='hosted') configuration profile; CreateHostedConfigurationVersion with a known JSON body (version 1).
5. CreateExtensionAssociation(ExtensionIdentifier=<ext>, ResourceIdentifier=arn:aws:appconfig:<region>:<account>:application/<app-id>/environment/<env-id>).
6. CreateDeploymentStrategy(DeploymentDurationInMinutes=1, GrowthFactor=100, GrowthType='LINEAR', FinalBakeTimeInMinutes=0, ReplicateTo='NONE').
7. StartDeployment(ConfigurationVersion='1', ...) and ReceiveMessage from the queue to read the delivered event.
EXPECTED: the event JSON contains TriggeredBy == 'USER' (alongside Application.Id, Environment.Id, ConfigurationProfile.Id, Type == 'PreStartDeployment', ContentVersion == '1', Content base64-decoding to version 1's bytes).
OBSERVED: the event JSON has no TriggeredBy key; a client reading event['TriggeredBy'] raises KeyError.
Re-replay appconfig__lambda.extension-action-point-invokes-lambda-during-deployment to confirm the fix (the check continues from there into the raising-target 400 veto leg, which is covered by the prior open issues named in the description).
