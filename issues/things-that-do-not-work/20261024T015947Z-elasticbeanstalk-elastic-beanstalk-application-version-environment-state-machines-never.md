Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticbeanstalk.eb-create-environment-with-a-cname-prefix-already-in-use-rejected elasticbeanstalk.eb-describe-instances-health-reports-the-environments-real-instances elasticbeanstalk.eb-restart-app-server-restarts-the-process-without-replacing-the-instance elasticbeanstalk.eb-retrieved-environment-info-carries-the-applications-own-log-line elasticbeanstalk.eb-rolling-deployment-keeps-serving-where-all-at-once-does-not elasticbeanstalk.eb-saved-configuration-template-reproduces-its-option-in-a-new-environment elasticbeanstalk.eb-update-environment-refuses-the-settings-validate-configuration-flags elasticbeanstalk__logs.eb-stream-logs-delivers-the-application-stdout-to-the-derived-log-group elasticbeanstalk.application-version-processing-reaches-failed-for-malformed-bundle elasticbeanstalk.compose-environments-creates-envs-from-versions-lifecycle elasticbeanstalk.e2e-compose-environments-from-templates-produces-linked-envs elasticbeanstalk.rebuild-terminated-environment-version-deleted-rejected elasticbeanstalk.deploy-application-version-via-cloudformation-then-environment-uses-it elasticbeanstalk.deploy-application-with-source-bundle-version-is-active elasticbeanstalk.deploy-application-cfn-then-swap-cnames-behaves elasticbeanstalk__sns__sqs.eb-environment-event-notification-is-delivered-to-the-subscribed-queue
Issue-Key: elasticbeanstalk.eb-create-environment-with-a-cname-prefix-already-in-use-rejected|run#16|AssertionError|assert st.upper() == "PROCESSED", f"version {label!r} must process, got {st!r}"
Issue-Key: elasticbeanstalk.eb-describe-instances-health-reports-the-environments-real-instances|_publish_version#6|AssertionError|assert status.upper() == "PROCESSED", (
Issue-Key: elasticbeanstalk.eb-restart-app-server-restarts-the-process-without-replacing-the-instance|_publish_version#6|AssertionError|assert status.upper() == "PROCESSED", (
Issue-Key: elasticbeanstalk.eb-retrieved-environment-info-carries-the-applications-own-log-line|_publish_version#6|AssertionError|assert status.upper() == "PROCESSED", (
Issue-Key: elasticbeanstalk.eb-rolling-deployment-keeps-serving-where-all-at-once-does-not|_publish_version#10|AssertionError|assert status.upper() == "PROCESSED", (
Issue-Key: elasticbeanstalk.eb-saved-configuration-template-reproduces-its-option-in-a-new-environment|_publish_version#9|AssertionError|assert status.upper() == "PROCESSED", (
Issue-Key: elasticbeanstalk.eb-update-environment-refuses-the-settings-validate-configuration-flags|run#17|AssertionError|assert st.upper() == "PROCESSED", f"version {label!r} must process, got {st!r}"
Issue-Key: elasticbeanstalk__logs.eb-stream-logs-delivers-the-application-stdout-to-the-derived-log-group|_publish_version#8|AssertionError|assert status.upper() == "PROCESSED", (
Issue-Key: elasticbeanstalk.application-version-processing-reaches-failed-for-malformed-bundle|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: elasticbeanstalk.compose-environments-creates-envs-from-versions-lifecycle|_wait_version_processed#21|Timeout|time.sleep(interval_s)
Issue-Key: elasticbeanstalk.e2e-compose-environments-from-templates-produces-linked-envs|_wait_version_processed#30|Timeout|time.sleep(interval_s)
Issue-Key: elasticbeanstalk.rebuild-terminated-environment-version-deleted-rejected|_wait_for_env_status#22|Timeout|time.sleep(interval_s)
Issue-Key: elasticbeanstalk.deploy-application-version-via-cloudformation-then-environment-uses-it|run#16|AssertionError|assert len(envs) == 1, f"expected exactly one environment named {env_name}, found {len(envs)}"
Issue-Key: elasticbeanstalk.deploy-application-with-source-bundle-version-is-active|run#16|AssertionError|assert len(versions) == 1, f"expected exactly one application version for {app_name}, found {len(versions)}"
Issue-Key: elasticbeanstalk.deploy-application-cfn-then-swap-cnames-behaves|_wait_for_env_status#14|Timeout|time.sleep(interval_s)
Issue-Key: elasticbeanstalk__sns__sqs.eb-environment-event-notification-is-delivered-to-the-subscribed-queue|_drain#22|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10,
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# Elastic Beanstalk application-version / environment state machines never advance: versions stay Unprocessed, ComposeEnvironments, RebuildEnvironment, CFN-provisioned EB resources and event notifications never materialize

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplicationVersion

## Description
Six distinct root causes in the elasticbeanstalk emulator, all in the same service, all grounded in failing behavioral checks of this suite. NOTE: two open issues cover ADJACENT ground but do NOT cover these checks (the suite marked their checks OWNED separately): 20261023T205650Z-elasticbeanstalk-elastic-beanstalk-deployment-substrate-is-missing-createstoragelocation-s.md and 20261023T205650Z-elasticbeanstalk-elastic-beanstalk-control-plane-guards-are-not-enforced.md. SIX causes must be closed before submitting.

Cause 1 of 6: CreateApplicationVersion leaves the version permanently in Status='Unprocessed'.
AWS: Smithy: CreateApplicationVersion -> ApplicationVersionDescriptionMessage/ApplicationVersionDescription, whose Status targets enum ApplicationVersionStatus with values Processed | Unprocessed | Failed | Processing | Building. Recorded behavior: a version created from a valid source bundle advances to 'Processed' (promptly when Process is not requested) and only a Processed version can be deployed to an environment.
Emulator: DescribeApplicationVersions keeps reporting Status='Unprocessed' for the whole poll window, e.g. "version 'v-04eb6ba2cf1b' must process cleanly before it can be deployed; DescribeApplicationVersions reports Status='UNPROCESSED'" and "version 'v1-3158a11186f9' must process, got 'UNPROCESSED'".
Why: every client that uploads a bundle and waits for the version to become deployable hangs forever; no deployment, rolling deployment, restart, log retrieval or saved-configuration flow can proceed past version creation.
Fix: the elasticbeanstalk application-version state transition in the CreateApplicationVersion handler and the version state it writes (services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/, plus the service's state module).
Traces: elasticbeanstalk.eb-create-environment-with-a-cname-prefix-already-in-use-rejected, elasticbeanstalk.eb-describe-instances-health-reports-the-environments-real-instances, elasticbeanstalk.eb-restart-app-server-restarts-the-process-without-replacing-the-instance, elasticbeanstalk.eb-retrieved-environment-info-carries-the-applications-own-log-line, elasticbeanstalk.eb-rolling-deployment-keeps-serving-where-all-at-once-does-not, elasticbeanstalk.eb-saved-configuration-template-reproduces-its-option-in-a-new-environment, elasticbeanstalk.eb-update-environment-refuses-the-settings-validate-configuration-flags, elasticbeanstalk__logs.eb-stream-logs-delivers-the-application-stdout-to-the-derived-log-group (this last check id carries the compound token 'elasticbeanstalk__logs'; the diverging operation is elasticbeanstalk's CreateApplicationVersion, so it is filed here).

Cause 2 of 6: a malformed source bundle never reaches the terminal Status='Failed'.
AWS: Smithy: ApplicationVersionStatus includes 'Failed'. Recorded behavior: a version whose source bundle is malformed is processed and lands in the terminal state Failed.
Emulator: the version never leaves its initial status, so the check polling for Failed never observes it and is killed by the harness ('Check timed out after 60.0s').
Why: a client cannot distinguish "still processing" from "bad bundle"; error handling for bad uploads never fires.
Fix: the same application-version processing/validation path as Cause 1 (terminal Failed branch).
Traces: elasticbeanstalk.application-version-processing-reaches-failed-for-malformed-bundle.

Cause 3 of 6: ComposeEnvironments never creates the environments described by the referenced versions' env.yaml manifests.
AWS: Smithy: ComposeEnvironments(ApplicationName, GroupName, VersionLabels) -> EnvironmentDescriptionsMessage{Environments}. Recorded behavior: one environment is created per referenced version manifest, each transitioning Launching -> Ready with the tier taken from the manifest.
Emulator: no composed environment ever appears under the application and the checks are killed at the harness timeout while polling for them.
Why: environment-group/composed-application clients get a silent no-op instead of environments.
Fix: the ComposeEnvironments handler and the environment state it must create.
Traces: elasticbeanstalk.compose-environments-creates-envs-from-versions-lifecycle, elasticbeanstalk.e2e-compose-environments-from-templates-produces-linked-envs.

Cause 4 of 6: RebuildEnvironment is accepted but moves no state.
AWS: Smithy: RebuildEnvironment(EnvironmentName|EnvironmentId) -> Unit, errors InsufficientPrivilegesException (403). Recorded behavior: on a live environment the call is accepted AND the environment leaves Ready (Updating/Terminating/Launching) before returning to Ready.
Emulator: the call returns 200 with an empty body and the environment never leaves Ready, so the check polling for the post-rebuild transition is killed at the harness timeout. Hypothesis: the handler validates and returns without scheduling any environment transition.
Why: a client that issues a rebuild and waits for the environment to cycle waits forever and cannot tell a rebuild happened at all.
Fix: the RebuildEnvironment handler and the environment status transition it must drive.
Traces: elasticbeanstalk.rebuild-terminated-environment-version-deleted-rejected.

Cause 5 of 6: CloudFormation-provisioned Elastic Beanstalk resources are never created in the EB service.
AWS: recorded behavior only — a CREATE_COMPLETE stack containing AWS::ElasticBeanstalk::ApplicationVersion / AWS::ElasticBeanstalk::Environment leaves exactly one application version visible via DescribeApplicationVersions and exactly one environment visible via DescribeEnvironments, and that environment reaches Ready.
Emulator: after the stack deploy, DescribeApplicationVersions reports 0 versions ("expected exactly one application version for sim2real-eb5-app-3c8ea0b44601, found 0") and DescribeEnvironments reports 0 environments ("expected exactly one environment named sim2real-eb-env-21dee5ad8ae5, found 0"); the CFN-then-swap-CNAMEs check is killed at the harness timeout waiting for the same resources.
Why: any infrastructure-as-code path to Elastic Beanstalk produces a stack that looks successful while no EB resource exists, so every follow-up EB call 404s.
Fix: the CloudFormation resource providers for the AWS::ElasticBeanstalk::* types and the EB state they must write. The open issue ...-deployment-substrate-is-missing-createstoragelocation-s.md owns two sibling checks but not these three.
Traces: elasticbeanstalk.deploy-application-version-via-cloudformation-then-environment-uses-it, elasticbeanstalk.deploy-application-with-source-bundle-version-is-active, elasticbeanstalk.deploy-application-cfn-then-swap-cnames-behaves.

Cause 6 of 6: environment event notifications are never published to the configured SNS topic.
AWS: recorded behavior only — with aws:elasticbeanstalk:sns:topics/Notification Endpoint (or Topic ARN) configured, environment lifecycle events are published to that SNS topic and reach a subscribed SQS queue.
Emulator: the subscribed queue receives nothing and the check is killed at the harness timeout ('Check timed out after 60.0s').
Why: event-driven consumers (ops alerting, deployment gating) never fire.
Fix: the EB event-emission path that must publish environment events to the configured SNS topic.
Traces: elasticbeanstalk__sns__sqs.eb-environment-event-notification-is-delivered-to-the-subscribed-queue (compound check token; the diverging publisher is elasticbeanstalk, so it is filed under that service).

## Evidence
Cause 1: expected Status='Processed' after CreateApplicationVersion; got AssertionError "version 'v-04eb6ba2cf1b' must process cleanly before it can be deployed; DescribeApplicationVersions reports Status='UNPROCESSED'" (and the same for v1-3158a11186f9, v-623dde5e3254, v-9cee95c682eb, v1-8fb7ea92567c, v-e5437354e09c, v1-b0f202856ae3, v-55440519c4f7).
Cause 2: expected the malformed-bundle version to reach Status='Failed'; got no transition, 'Error: Check timed out after 60.0s'.
Cause 3: expected one composed environment per manifest reaching Ready; got none within the poll window, 'Error: Check timed out after 60.0s'.
Cause 4: expected the environment to leave Ready after RebuildEnvironment; got a 200 empty response and no transition, 'Error: Check timed out after 60.0s'.
Cause 5: expected exactly one CFN-created application version / environment; got AssertionError 'expected exactly one application version for sim2real-eb5-app-3c8ea0b44601, found 0' and 'expected exactly one environment named sim2real-eb-env-21dee5ad8ae5, found 0'.
Cause 6: expected the environment event on the subscribed SQS queue; got nothing, 'Error: Check timed out after 60.0s'.

## Steps to Reproduce
1. Boot the emulator and point an AWS client at it (region us-west-2).
2. Cause 1/2: eb.create_application; eb.create_storage_location; s3.put_object of a bundle; eb.create_application_version(ApplicationName, VersionLabel, SourceBundle) then poll eb.describe_application_versions -> Status stays 'Unprocessed' (and a malformed bundle never reaches 'Failed').
3. Cause 3: create two versions whose bundles carry env.yaml manifests, call eb.compose_environments(ApplicationName, GroupName, VersionLabels) and poll eb.describe_environments -> no environment appears.
4. Cause 4: create an environment, wait for Ready, call eb.rebuild_environment(EnvironmentName) and poll eb.describe_environments -> Status never leaves Ready.
5. Cause 5: deploy a CloudFormation stack containing AWS::ElasticBeanstalk::ApplicationVersion and AWS::ElasticBeanstalk::Environment, wait for CREATE_COMPLETE, then call eb.describe_application_versions / eb.describe_environments -> 0 results.
6. Cause 6: configure aws:elasticbeanstalk:sns:topics Notification Endpoint to an SNS topic with an SQS subscription, create/update an environment, poll the queue -> no messages.
7. Re-replay the checks listed in trace_ids.
