Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__sqs.deployment-events-to-sqs-extension-delivers-lifecycle-messages
Issue-Key: appconfig__sqs.deployment-events-to-sqs-extension-delivers-lifecycle-messages|_drain#36|Timeout|msgs = sqs.receive_message(
Issue-Commit: c3a6f169c346304d6b4c388bf8378b1c143b9d70

# appconfig__sqs: the AWS-authored "deployment events to Amazon SQS" extension still delivers no lifecycle notification, so the association's queueArn is never sent to

> _Note: this issue was identified against the code base at commit `c3a6f169c346304d6b4c388bf8378b1c143b9d70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDeployment

## Description
One net-new root cause (Cause 1 of 1).

Failing check id: appconfig__sqs.deployment-events-to-sqs-extension-delivers-lifecycle-messages.

PRIOR WORK, read first: open issues 20261101T232607Z-appconfig-sqs-appconfig-s-aws-authored-deployment-event-notification-extensions.md and 20261025T034405Z-appconfig-sqs-appconfig-s-predefined-deployment-events-to-amazon-sqs.md both name this check id, but neither carries this replay's key for it and the report marks this check neither OWNED nor ALREADY FILED. The symptom is unchanged across replays, so the authored-extension delivery path should be re-verified end to end rather than assumed to exist; the first of those issues located the cause in source and its analysis is directly reusable.

AWS: an AWS-authored notification extension (ARN with an EMPTY account field, e.g. arn:aws:appconfig:<region>::extension/AWS.AppConfig.DeploymentEventsToSqs/1) binds exactly the three deployment-lifecycle action points ON_DEPLOYMENT_START, ON_DEPLOYMENT_COMPLETE and ON_DEPLOYMENT_ROLLED_BACK and is not customizable. Its DESTINATION is not part of the extension: Smithy documents Parameter as "A value such as an Amazon Resource Name (ARN) or an Amazon Simple Notification Service topic entered in an extension when invoked. Parameter values are specified in an extension association", and Action$Uri ("The extension URI associated to the action point in the extension definition") is OPTIONAL — an authored notification extension publishes none. So associating that extension to an environment ARN with Parameters={'queueArn': <queue arn>} makes AppConfig enqueue one message when a deployment starts and one when it completes (StartDeployment is POST /applications/{ApplicationId}/environments/{EnvironmentId}/deployments, success 201; CreateExtensionAssociation is POST /extensionassociations with a ParameterValueMap). Recorded behavior only for the body keys: each body carries Type in PascalCase ('OnDeploymentStart' / 'OnDeploymentComplete' — not the SCREAMING_SNAKE action-point keys), DeploymentNumber, ConfigurationVersion, Application.Id, Environment.Id, ConfigurationProfile.Id and the association's Parameters (echoing queueArn). Deleting the association stops delivery — GetExtensionAssociation then answers ResourceNotFoundException with the message template "ExtensionAssociation could not be found for account <account>." (fixed text, with the account id taken from the caller's identity) — and re-creating it restores delivery for the NEXT deployment only.

Emulator: nothing is ever delivered, and the check is killed before any assertion — "Error: Check timed out after 180.0s". Its only unbounded waits are the deploy-and-wait-complete polls and the queue drains, so the positive leg (two notifications), the absence leg and the restore leg are all ungraded. Hypothesis (source-located by the prior issue and consistent with this replay): the authored extensions are published in the catalogue with actions that carry a name and no Uri, while action dispatch skips every action whose Uri is absent or blank, so no target is ever built for an authored notification extension and the association is pure metadata even though the generic SQS send path beside it works.

Why: this predefined extension is the supported, zero-code way to observe AppConfig releases from outside AppConfig. A consumer driving release gating, change logging or paging off those SQS messages receives nothing and never learns that a deployment started, completed or rolled back. It is worse than a silent gap: because no delivery path exists at all, the "association deleted, therefore nothing arrives" expectation passes for the wrong reason, so a client can never detect that its notification wiring is broken either.

Fix: the authored-extension branch of AppConfig's extension-action dispatch (the action-target resolution that drops actions with no Uri, and the invoke path it feeds), plus the deployment state machine that reaches those action points. An action belonging to an AWS-authored notification extension must resolve its destination from the bound association's parameter value (queueArn for ...ToSqs, the topic ARN for ...ToSns, the default event bus for ...ToEventBridge) instead of from Action$Uri, and must send the lifecycle body (PascalCase Type, DeploymentNumber, ConfigurationVersion, Application/Environment/ConfigurationProfile, and the association's Parameters) at each of the extension's three action points — and send nothing once the association is deleted.

Traces: appconfig__sqs.deployment-events-to-sqs-extension-delivers-lifecycle-messages. Queue URLs/ARNs, the account id, generated names and the particular configuration bytes in that run are per-run values that only fill the templates; the requirements are the three action points, delivery of the documented body to the destination the association names, the absence of delivery once the association is deleted, and restoration after re-association.

Note on budget for the fixer: the check drives three deployments of a DeploymentDurationInMinutes=1 strategy and declares _NOTIFICATION_BUDGET_S=900 / _DEPLOY_TIMEOUT_S=300, so confirming the fix needs a wall-clock budget well above the 180 s per-check cap it was killed at; the steps below grade the same contract directly.

## Evidence
Behavioral replay of suite simparity-behavioral-aws-v1 against the booted emulator: 205/240 passed (85.4%), 28 mismatched, 7 errored, 0 skipped; bootstrap_signal false (the emulator answered throughout, so this is not a bootstrap failure). Per-service appconfig__sqs 0/1 (0%), 1 failed.

[mismatch] appconfig__sqs.deployment-events-to-sqs-extension-delivers-lifecycle-messages — Error: Check timed out after 180.0s. The check reaches no assertion; its only unbounded waits are _deploy_and_wait_complete (StartDeployment, then poll GetDeployment until State is one of COMPLETE/ROLLED_BACK/REVERTED) and _drain (long-poll the queue for the expected messages), so nothing past the association is graded.

What the check sets up and would then assert, from its source in the replay report:
  - CreateQueue, read QueueArn, and set a queue Policy allowing Principal {"Service": "appconfig.amazonaws.com"} action SQS:SendMessage on it with Condition aws:SourceAccount == <account>.
  - Discover the AWS-authored SQS notification extension from ListExtensions (ARN containing '::extension/') and assert GetExtension(...)['Actions'] == {ON_DEPLOYMENT_START, ON_DEPLOYMENT_COMPLETE, ON_DEPLOYMENT_ROLLED_BACK}.
  - CreateExtensionAssociation(ExtensionIdentifier=<authored arn>, ResourceIdentifier=<environment arn>, Parameters={'queueArn': <queue arn>}) and assert GetExtensionAssociation(...)['Parameters']['queueArn'] == the queue ARN.
  - Positive leg: deploy version 1 with a DeploymentDurationInMinutes=1 strategy, then expect exactly 2 messages whose Type set == {'OnDeploymentStart', 'OnDeploymentComplete'} ("the two messages must be the start and complete notifications, PascalCase — NOT the SCREAMING_SNAKE action-point keys"), each with str(DeploymentNumber) == that deployment, str(ConfigurationVersion) == the version deployed, Application.Id / Environment.Id / ConfigurationProfile.Id equal to the ids, and Parameters['queueArn'] echoing the queue ARN; and the data plane serving that version's exact bytes.
  - Absence leg: DeleteExtensionAssociation, then GetExtensionAssociation must answer Code 'ResourceNotFoundException' with Message 'ExtensionAssociation could not be found for account <account>.'; deploy version 2 (proved to have really deployed at the data plane) and the queue must stay empty for 3x the delivery latency measured in the same run, with ApproximateNumberOfMessages == '0'.
  - Restore leg: re-associate, deploy version 3, and the same two notification types must come back naming the NEW deployment number.
Expected (AWS): the two notifications above on the first and third deployments, and none on the second.
Actual (emulator): nothing is ever enqueued, and the check is killed at the 180 s cap without producing any body to compare.

Smithy consulted (services/service-com-amazonaws-appconfig-amazonappconfig/smithy-model/com-amazonaws-appconfig-amazonappconfig.json): StartDeployment — POST /applications/{ApplicationId}/environments/{EnvironmentId}/deployments, code 201, output Deployment, errors BadRequestException 400 / ConflictException 409 / ResourceNotFoundException 404 / InternalServerException 500; CreateExtensionAssociation — POST /extensionassociations with Parameters (ParameterValueMap); Action$Uri optional; Parameter documented as supplied by the association. The notification body keys and the PascalCase Type are recorded behavior, not modelled.

Both constituent services are stood up in this checkout (appconfig, sqs), so this is a fidelity finding, not a bootstrap gap.

## Steps to Reproduce
Boot the emulator and point appconfig, appconfigdata and sqs clients at its data listener.
1. CreateQueue; GetQueueAttributes for QueueArn; SetQueueAttributes with a Policy allowing Principal {"Service":"appconfig.amazonaws.com"} SQS:SendMessage on that ARN (Condition aws:SourceAccount == your account).
2. ListExtensions and pick the AWS-authored entry whose ARN contains '::extension/' and whose name names SQS; GetExtension on it — Actions must be exactly {ON_DEPLOYMENT_START, ON_DEPLOYMENT_COMPLETE, ON_DEPLOYMENT_ROLLED_BACK} with a required queue-ARN parameter.
3. CreateApplication / CreateEnvironment / CreateConfigurationProfile(LocationUri='hosted'); CreateHostedConfigurationVersion three times with distinguishable bytes (versions 1, 2, 3).
4. CreateDeploymentStrategy with a NON-ZERO DeploymentDurationInMinutes (1 is enough; a zero-duration/zero-bake deployment invokes no action point), GrowthFactor=100, GrowthType='LINEAR', FinalBakeTimeInMinutes=0, ReplicateTo='NONE'.
5. CreateExtensionAssociation(ExtensionIdentifier=<authored arn>, ResourceIdentifier=arn:aws:appconfig:<region>:<account>:application/<app-id>/environment/<env-id>, Parameters={'queueArn': <queue arn>}); GetExtensionAssociation must echo the queue ARN.
6. StartDeployment of version 1 and poll GetDeployment to State 'COMPLETE'; ReceiveMessage on the queue.
   EXPECTED (AWS): two bodies with Type 'OnDeploymentStart' and 'OnDeploymentComplete', each naming that DeploymentNumber and ConfigurationVersion, the Application.Id, Environment.Id and ConfigurationProfile.Id, and echoing Parameters['queueArn'].
   OBSERVED: nothing is ever enqueued.
7. DeleteExtensionAssociation; GetExtensionAssociation must answer ResourceNotFoundException with Message 'ExtensionAssociation could not be found for account <account>.'. Deploy version 2: the deployment must still complete and reach the data plane, and NOTHING must be enqueued (ApproximateNumberOfMessages '0'). This leg currently 'passes' only because no delivery path exists.
8. Re-associate and deploy version 3: the two notifications must come back naming the NEW deployment number.
Re-replay appconfig__sqs.deployment-events-to-sqs-extension-delivers-lifecycle-messages with a wall-clock budget at least its declared 900 s to confirm; under the current 180 s cap it cannot finish three 1-minute deployments, so grade steps 6-8 directly as well.
