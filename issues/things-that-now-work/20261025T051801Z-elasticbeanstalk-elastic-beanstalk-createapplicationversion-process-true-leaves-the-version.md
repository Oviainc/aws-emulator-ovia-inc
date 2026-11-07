Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticbeanstalk__sns__sqs.eb-environment-event-notification-is-delivered-to-the-subscribed-queue
Issue-Key: elasticbeanstalk__sns__sqs.eb-environment-event-notification-is-delivered-to-the-subscribed-queue|_publish_version#27|AssertionError|assert status.upper() == "PROCESSED", (
Issue-Commit: 0d71bc8965f792a59de4932140e9e92304a856f9

# Elastic Beanstalk CreateApplicationVersion(Process=true) leaves the version at Status='Unprocessed' forever, so the SNS/SQS environment-event notification flow can never reach its first deployable version

> _Note: this issue was identified against the code base at commit `0d71bc8965f792a59de4932140e9e92304a856f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplicationVersion

## Description
One root cause, grounded in one failing behavioral check of this suite. NOTE on adjacency: the open issue 20261024T015947Z-elasticbeanstalk-elastic-beanstalk-application-version-environment-state-machines-never.md describes the same application-version stall as its Cause 1 and even lists this check id, but the replay key it carries for THIS check records a later-stage failure (`_drain#22|Timeout|sqs.receive_message`, i.e. the queue drain), so the suite did not mark this failure OWNED: the check now fails much earlier, at version processing, and that earlier failure is filed here so it is replayed and gated. A fixer closing that issue should treat this as the same application-version transition and re-replay the check id below.

Cause 1 of 1: CreateApplicationVersion with Process=true never advances the application version out of Status='Unprocessed'.
AWS: Smithy: CreateApplicationVersion(ApplicationName, VersionLabel required; SourceBundle: S3Location; Process: ApplicationVersionProccess boolean) -> ApplicationVersionDescriptionMessage{ApplicationVersion: ApplicationVersionDescription}; ApplicationVersionDescription.Status targets enum ApplicationVersionStatus with values Processed | Unprocessed | Failed | Processing | Building; DescribeApplicationVersions(ApplicationName, VersionLabels) -> ApplicationVersionDescriptionsMessage{ApplicationVersions} with no modeled errors. Recorded behavior: a version created from a valid source bundle in the CreateStorageLocation bucket is processed and settles in the terminal state 'Processed' (passing through 'Processing' when Process=true), and only a Processed version can be deployed to an environment; a version that cannot be processed settles in the terminal state 'Failed'. The transition is a requirement; the wall-clock time it takes and the version label are not.
Emulator: the version is created, but every subsequent DescribeApplicationVersions for that label keeps reporting Status='UNPROCESSED' for the whole 300s poll window. The emulator never emits 'Processing' and never reaches either terminal state, so no caller can distinguish "queued", "still working" and "will never process".
Why: a client that uploads a source bundle and waits for the version to become deployable — which is the only supported way to deploy to Elastic Beanstalk — hangs indefinitely and never gets to CreateEnvironment/UpdateEnvironment. In this check it means the whole downstream behavior under test (environment lifecycle events published to the SNS topic configured through aws:elasticbeanstalk:sns:topics and delivered to a subscribed SQS queue) is unreachable: the failure is now the version stall, not the delivery, so the notification leg is still unverified behind it.
Fix: the application-version state transition in the CreateApplicationVersion handler and the version state it writes (the elasticbeanstalk service's ops module for create_application_version plus the service state module that DescribeApplicationVersions reads), so a created version advances to a terminal ApplicationVersionStatus ('Processed' for a valid bundle, 'Failed' otherwise) rather than remaining 'Unprocessed'.
Traces: elasticbeanstalk__sns__sqs.eb-environment-event-notification-is-delivered-to-the-subscribed-queue. Service attribution: this check id carries the compound token 'elasticbeanstalk__sns__sqs' (its SERVICES are elasticbeanstalk, sns, sqs); the diverging operation is elasticbeanstalk's CreateApplicationVersion/DescribeApplicationVersions, so the issue is filed under service 'elasticbeanstalk'.

## Evidence
Expected: after eb.create_application_version(ApplicationName=app, VersionLabel=label, SourceBundle={S3Bucket, S3Key}, Process=True), eb.describe_application_versions(ApplicationName=app, VersionLabels=[label])['ApplicationVersions'][0]['Status'] reaches the terminal value 'Processed' (ApplicationVersionStatus), after optionally passing through 'Processing'.

Actual (replay of elasticbeanstalk__sns__sqs.eb-environment-event-notification-is-delivered-to-the-subscribed-queue):
  File "check_elasticbeanstalk__sns__sqs_eb_environment_event_notification_is_delivered_to_the_subscribed_queue.py", line 524, in run
    label, _bucket, _key = _publish_version(ctx, eb, s3, app, marker, nonce)
  File "...", line 373, in _publish_version
    assert status.upper() == "PROCESSED", (
  AssertionError: version 'v-742e7e549eba' must process cleanly before it can be deployed; DescribeApplicationVersions reports Status='UNPROCESSED'

The status stayed 'UNPROCESSED' across the check's bounded 300s poll (5s interval); 'Processing' was never observed, so this is a missing transition rather than a slow one. The version label and the generated bucket/key are examples only.

## Steps to Reproduce
1. Boot the emulator and point an AWS client at it (region us-west-2).
2. eb.create_application(ApplicationName=app).
3. bucket = eb.create_storage_location()['S3Bucket']; s3.put_object(Bucket=bucket, Key='sim2real-eb/<app>/<label>.zip', Body=<valid zip with Procfile + server.py>).
4. eb.create_application_version(ApplicationName=app, VersionLabel=label, SourceBundle={'S3Bucket': bucket, 'S3Key': key}, Process=True).
5. Poll eb.describe_application_versions(ApplicationName=app, VersionLabels=[label])['ApplicationVersions'][0]['Status'] every 5s for 300s -> it stays 'Unprocessed'; it never becomes 'Processing' and never reaches 'Processed' (or 'Failed' for a deliberately malformed bundle).
6. Re-replay elasticbeanstalk__sns__sqs.eb-environment-event-notification-is-delivered-to-the-subscribed-queue to confirm the fix; note the check then proceeds to the environment-event delivery assertions, which this stall has been masking.
