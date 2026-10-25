Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__sqs.deployment-events-to-sqs-extension-delivers-lifecycle-messages
Issue-Key: appconfig__sqs.deployment-events-to-sqs-extension-delivers-lifecycle-messages|_deploy_and_wait_complete#35|Timeout|time.sleep(3.0)
Issue-Commit: 9181cd543fe84fac851a491e53a22f2746df95d8

# AppConfig's predefined "deployment events to Amazon SQS" extension delivers no lifecycle notifications and the deployment it should notify about never settles

> _Note: this issue was identified against the code base at commit `9181cd543fe84fac851a491e53a22f2746df95d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDeployment

## Description
One net-new root cause.

Cause 1 of 1: the AWS-authored AppConfig extension that enqueues deployment lifecycle notifications to SQS produces nothing, and the deployment it is bound to never reaches a terminal state.
AWS (per the check's hypotheses from the AppConfig user guide's predefined-SQS-notification page; this check is marked DRAFT/unblessed, so the exact extension name and payload keys are hypotheses the first replay must confirm — the check deliberately DISCOVERS the name from ListExtensions rather than hardcoding it): every account sees an AWS-authored extension (ARN of the form arn:aws:appconfig::extension/..., i.e. no account id) whose Actions are exactly {ON_DEPLOYMENT_START, ON_DEPLOYMENT_COMPLETE, ON_DEPLOYMENT_ROLLED_BACK} and which is not customizable. Associating it to an environment ARN with Parameters={'queueArn': <queue arn>} makes AppConfig enqueue one message on deployment start and one on deployment complete; each body carries Type ('OnDeploymentStart'/'OnDeploymentComplete', PascalCase — not the SCREAMING_SNAKE action-point keys), DeploymentNumber, ConfigurationVersion, Application.Id, Environment.Id, ConfigurationProfile.Id and the association's Parameters. Deleting the association stops delivery (and GetExtensionAssociation then answers ResourceNotFoundException); re-creating it restores delivery for the next deployment. Smithy: StartDeployment POST /applications/{ApplicationId}/environments/{EnvironmentId}/deployments, success 201, output Deployment with State (DeploymentState enum) and AppliedExtensions; CreateExtensionAssociation POST /extensionassociations, Parameters (ParameterValueMap, len 0..10).
Emulator: the check never completes — it is killed at the 60 s harness cap. Its only unbounded waits are the deploy-and-wait helper (start_deployment then poll GetDeployment until State is one of COMPLETE/ROLLED_BACK/REVERTED) and the queue drains, so the emulator neither settles the deployment nor enqueues any lifecycle notification; the positive, absence and restore beats are therefore all unobservable. Hypothesis: extension associations are stored as metadata only, with no ON_DEPLOYMENT_* dispatch from the deployment state machine, and a non-zero-duration deployment never progresses to a terminal state.
Why: this predefined extension is the supported way to observe AppConfig releases from outside AppConfig — a consumer that drives rollout gating, change logging or paging off those SQS messages receives nothing and never learns a deployment started or completed, while the deployment itself stays in flight forever (which also blocks teardown, since the environment never leaves Deploying).
Fix: AppConfig's extension catalogue and action-point dispatch, plus the deployment state machine in the StartDeployment handler and its deployment/environment state: the AWS-authored SQS notification extension must exist in ListExtensions/GetExtension with its three fixed action points and its queueArn parameter, the deployment state machine must progress a non-zero-duration deployment to COMPLETE, and each ON_DEPLOYMENT_* transition must enqueue the documented body to the queue named by the bound association's parameter — and enqueue nothing once the association is deleted. The never-settling-deployment symptom is already filed for the CloudWatch Monitors path as 20261025T020208Z-appconfig-cloudwatch-appconfig-deployments-with-cloudwatch-alarm-monitors-never-reach.md; this deployment carries NO Monitors, so that issue does not cover it.
Traces: appconfig__sqs.deployment-events-to-sqs-extension-delivers-lifecycle-messages.

## Evidence
Behavioral replay, suite simparity-behavioral-ga-prod-v1. appconfig__sqs.deployment-events-to-sqs-extension-delivers-lifecycle-messages [mismatch]: 'Error: Check timed out after 60.0s' — the check reached no assertion. Its helper _deploy_and_wait_complete starts a deployment with a non-zero-duration strategy and polls GetDeployment for State in ('COMPLETE','ROLLED_BACK','REVERTED'), failing with 'deployment <n> of version <v> did not reach COMPLETE within <timeout>s; last state <state>'; the delivery legs would then have asserted two messages with types exactly {'OnDeploymentStart','OnDeploymentComplete'}, each naming the deployment number, configuration version, application, environment, configuration profile and echoing Parameters['queueArn'], followed by an absence beat (association deleted -> empty queue and ApproximateNumberOfMessages == '0') and a restore beat. Queue URLs/ARNs, ids and generated names in the run are examples; the requirements are the extension's existence and action points, the terminal deployment state, and the delivery/absence behavior.

## Steps to Reproduce
1. create_queue and read its QueueArn.
2. list_extensions and select the AWS-authored entry (ARN containing '::extension/') whose name mentions SQS; get_extension on it -> expect exactly one such extension, with Actions == {ON_DEPLOYMENT_START, ON_DEPLOYMENT_COMPLETE, ON_DEPLOYMENT_ROLLED_BACK}.
3. create_application / create_environment / create_configuration_profile(LocationUri='hosted') and three hosted configuration versions.
4. create_deployment_strategy with a non-zero DeploymentDurationInMinutes.
5. create_extension_association(ExtensionIdentifier=<that extension ARN>, ResourceIdentifier=<environment ARN>, Parameters={'queueArn': <queue arn>}).
6. start_deployment for version 1 and poll get_deployment -> expect State COMPLETE; receive_message on the queue -> expect two bodies with Type 'OnDeploymentStart' and 'OnDeploymentComplete' naming that deployment number and configuration version. Observed: the deployment never leaves its transitional state and the queue stays empty until the harness kills the check.
7. delete_extension_association, deploy version 2 -> expect the deployment to complete and NOTHING to be enqueued; re-associate and deploy version 3 -> expect the two notifications to come back naming the new deployment.

Re-replay the check in trace_ids to confirm the cause is closed.
