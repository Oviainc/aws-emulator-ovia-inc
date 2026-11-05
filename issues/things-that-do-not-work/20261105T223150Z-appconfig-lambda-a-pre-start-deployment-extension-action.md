Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__lambda.extension-action-point-invokes-lambda-during-deployment
Issue-Key: appconfig__lambda.extension-action-point-invokes-lambda-during-deployment|run#45|Timeout|assert dep1 is not None, (
Issue-Commit: 2730cd672dd608c80462ed6cac2962eb48b48789

# appconfig__lambda: a PRE_START_DEPLOYMENT extension action point never invokes its Lambda, so StartDeployment is never gated and the action-point scenario never settles

> _Note: this issue was identified against the code base at commit `2730cd672dd608c80462ed6cac2962eb48b48789`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDeployment

## Description
Cause 1 of 1: an AppConfig extension whose Actions declare a PRE_START_DEPLOYMENT action with a Lambda Uri, associated to an environment, is accepted by the control plane but the action point is never dispatched when StartDeployment runs — so the hook neither gates nor vetoes a deployment, and the scenario never reaches a verdict.

AWS: Smithy (service-com-amazonaws-appconfig-amazonappconfig): CreateExtension input Actions is an @required ActionsMap of length 1..5, keyed by action point; StartDeployment is POST /applications/{ApplicationId}/environments/{EnvironmentId}/deployments with success code 201 and modelled errors BadRequestException (400), ConflictException (409), ResourceNotFoundException (404), InternalServerException (500). AWS's documented behavior for a PRE_START_DEPLOYMENT action: StartDeployment synchronously invokes the action's Uri with an event naming the deployment under consideration — Type "PreStartDeployment" (CamelCase, against the SCREAMING_SNAKE 'PRE_START_DEPLOYMENT' key used on the request), Application.Id and Environment.Id and ConfigurationProfile.Id equal to the ids on the request, ContentVersion equal to the ConfigurationVersion on the request, Content base64-encoding the exact candidate configuration bytes, and TriggeredBy "USER" for a customer-issued StartDeployment. If the target returns normally the deployment is created (201) and runs to State COMPLETE; if the target raises, StartDeployment is refused with BadRequestException (HTTP 400) whose message is, as a template, "Error invoking extension <extension-name>: Unhandled error, Lambda Request ID: <lambda-request-id>" — fixed text "Error invoking extension ", ": Unhandled error, Lambda Request ID: ", with the extension name taken from the request's extension and the request id taken from the invocation that failed — no deployment record is created, and the data plane keeps serving the previously deployed version.

Emulator: the action point is never invoked. The scenario is killed by the harness at its budget with "Error: Check timed out after 60.0s": the StartDeployment barrier loop (which retries only on BadRequestException while the invoke role's trust propagates) never gets a deployment started, so nothing ever reaches the observation queue the Lambda forwards its event to, and the refusal leg — a raising target blocking StartDeployment with BadRequestException and that exact message — is never reached either. No event, no 201 and no 400 are produced for comparison.

Why: PRE_START_DEPLOYMENT is the one hook a customer uses to veto a release. A client relying on AWS's behavior gets a configuration gate that is purely decorative: either releases never start at all, or (for an emulator that treats action points as fire-and-forget) content the gate was written to reject reaches the data plane with no refusal. Either way the pre-flight approval, the content handed to the approver, and the refusal a failing approver must produce are all unobservable.

Fix: the AppConfig deployment pipeline's extension action-point dispatch — the StartDeployment handler must resolve extension associations for the target environment, synchronously invoke each matching action's Uri through the action's RoleArn with the documented event body, and map a target failure onto BadRequestException with AWS's "Error invoking extension ..." message while creating no deployment record.

Traces: appconfig__lambda.extension-action-point-invokes-lambda-during-deployment. The open issue 20261105T111951Z-appconfig-lambda-appconfig-extension-action-points-never-invoke.md describes a closely related symptom but does not carry this check's replay key (it owns pre-create-hosted-configuration-version-extension-transforms-stored-content and validator-lambda-rejects-noncompliant-content-rejected); see also 20261027T010737Z-appconfig-lambda-backed-extension-action-points-still-never.md and 20261103T031416Z-appconfig-lambda-2-net-new-causes-startdeployment-ignores.md.

## Evidence
Expected (AWS):
  - StartDeployment -> 201, and the PRE_START_DEPLOYMENT target is invoked with an event where Type == 'PreStartDeployment', Application.Id/Environment.Id/ConfigurationProfile.Id equal the request's ids, ContentVersion == the request's ConfigurationVersion, base64decode(Content) == the exact candidate configuration bytes, TriggeredBy == 'USER'.
  - deployment reaches State COMPLETE and the data plane serves those bytes byte-exactly.
  - with the target raising: StartDeployment -> BadRequestException (HTTP 400), message 'Error invoking extension <extension-name>: Unhandled error, Lambda Request ID: <lambda-request-id>'; ListDeployments still shows only the earlier deployment; the data plane still serves the previous version.

Actual (emulator): 'Error: Check timed out after 60.0s'. The StartDeployment barrier never succeeded, nothing was delivered to the observation queue, and neither the 201 path nor the 400 refusal path was exercised. CreateExtension and CreateExtensionAssociation were accepted (VersionNumber 1, Actions and ResourceArn round-tripped), so the association exists but is inert.

## Steps to Reproduce
1. Create a Lambda function that forwards its event to an SQS queue and raises when its REFUSE env var is '1'; grant appconfig.amazonaws.com lambda:InvokeFunction on it and create a role appconfig can assume that allows lambda:InvokeFunction on the function ARN.
2. CreateExtension with Actions={'PRE_START_DEPLOYMENT': [{'Name':'gate','Uri':<fn-arn>,'RoleArn':<role-arn>}]}.
3. Create an application, an environment and a hosted configuration profile; CreateHostedConfigurationVersion twice (versions 1 and 2).
4. CreateExtensionAssociation binding the extension to arn:aws:appconfig:<region>:<account>:application/<app-id>/environment/<env-id>.
5. CreateDeploymentStrategy with DeploymentDurationInMinutes=1, GrowthFactor=100, GrowthType=LINEAR, FinalBakeTimeInMinutes=0 (a non-instant strategy).
6. With REFUSE='0', StartDeployment for version 1: expect 201 and an event on the queue with Type 'PreStartDeployment' and Content base64-decoding to version 1's bytes. Observe no event arrives.
7. Set REFUSE='1' and StartDeployment for version 2: expect BadRequestException with 'Error invoking extension <name>: Unhandled error, Lambda Request ID: <id>', no new deployment record, and version 1 still served.
