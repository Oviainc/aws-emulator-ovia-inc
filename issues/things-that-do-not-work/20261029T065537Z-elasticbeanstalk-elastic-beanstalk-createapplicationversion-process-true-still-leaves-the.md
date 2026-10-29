Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticbeanstalk__sns__sqs.eb-environment-event-notification-is-delivered-to-the-subscribed-queue
Issue-Key: elasticbeanstalk__sns__sqs.eb-environment-event-notification-is-delivered-to-the-subscribed-queue|_publish_version#25|AssertionError|assert status.upper() == "PROCESSED", (
Issue-Commit: 6c829b3943454ada5df5dfe2630a7a4c6694ce1c

# Elastic Beanstalk CreateApplicationVersion(Process=True) still leaves the version at Status='UNPROCESSED', so no version ever becomes deployable

> _Note: this issue was identified against the code base at commit `6c829b3943454ada5df5dfe2630a7a4c6694ce1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplicationVersion

## Description
One root cause, grounded in one failing behavioral check.

Cause 1 of 1: CreateApplicationVersion with Process=True never advances the application version out of Status='Unprocessed'.

AWS: Smithy (com.amazonaws.elasticbeanstalk): CreateApplicationVersion(ApplicationName, VersionLabel required; SourceBundle: S3Location; Process: boolean) -> ApplicationVersionDescriptionMessage{ApplicationVersion: ApplicationVersionDescription}. ApplicationVersionDescription.Status targets the enum ApplicationVersionStatus with values Processed | Unprocessed | Failed | Processing | Building, documented as "The processing status of the application version... Many of the values are only applicable if you specified True for the Process parameter": Unprocessed - "Application version wasn't pre-processed or validated"; Processing - "Elastic Beanstalk is currently processing the application version"; Processed - "Elastic Beanstalk was successfully pre-processed and validated"; Failed - "configuration files didn't pass validation. This application version isn't usable". DescribeApplicationVersions(ApplicationName, VersionLabels) -> ApplicationVersionDescriptionsMessage{ApplicationVersions} has no modeled errors. Recorded behavior: a version created with Process=True from a valid source bundle in the CreateStorageLocation bucket passes through 'Processing' and settles in the terminal state 'Processed'; only a Processed version can be deployed to an environment, and an unprocessable bundle settles in the terminal state 'Failed'. The transition to a terminal state is the requirement; the wall-clock time it takes and the version label are not.

Emulator: the version is created, but every DescribeApplicationVersions for that label keeps reporting Status='UNPROCESSED' for the whole 300 s bounded poll. 'PROCESSING' is never observed and neither terminal state is ever reached, so this is a missing transition rather than a slow one, and no caller can distinguish "queued", "still working" and "will never process".

Why: a client that uploads a source bundle and waits for the version to become deployable - the only supported way to deploy to Elastic Beanstalk - hangs forever and never reaches CreateEnvironment/UpdateEnvironment. In this check it also means the behavior actually under test (environment lifecycle events published to the SNS topic configured through aws:elasticbeanstalk:sns:topics and delivered to a subscribed SQS queue) is unreachable and still unverified behind this stall.

Fix: the application-version state transition in the elasticbeanstalk service - the CreateApplicationVersion handler and the version state DescribeApplicationVersions reads - so a created version advances to a terminal ApplicationVersionStatus ('Processed' for a valid bundle, 'Failed' otherwise), passing through 'Processing' when Process=True, rather than remaining 'Unprocessed'.

Related open issues that do not carry this replay key: 20261025T051801Z-elasticbeanstalk-elastic-beanstalk-createapplicationversion-process-true-leaves-the-version.md and 20261024T015947Z-elasticbeanstalk-elastic-beanstalk-application-version-environment-state-machines-never.md both describe this same application-version stall, but the replay key each carries for this check records a different frame offset inside _publish_version, so the suite did not mark this failure OWNED. A fixer closing either should treat this as the same transition and re-replay the check id below.

Traces: elasticbeanstalk__sns__sqs.eb-environment-event-notification-is-delivered-to-the-subscribed-queue.

Service attribution: the check id carries the compound token 'elasticbeanstalk__sns__sqs' (its SERVICES are elasticbeanstalk, sns, sqs); the diverging operation is elasticbeanstalk's CreateApplicationVersion/DescribeApplicationVersions, so this is filed under service 'elasticbeanstalk'.

## Evidence
Expected (AWS): after eb.create_application_version(ApplicationName=app, VersionLabel=label, SourceBundle={'S3Bucket': <CreateStorageLocation bucket>, 'S3Key': key}, Process=True), eb.describe_application_versions(ApplicationName=app, VersionLabels=[label])['ApplicationVersions'][0]['Status'] reaches the terminal ApplicationVersionStatus 'Processed' (case-folded 'PROCESSED' on the wire), optionally after passing through 'Processing'.

Actual (emulator):
[mismatch] elasticbeanstalk__sns__sqs.eb-environment-event-notification-is-delivered-to-the-subscribed-queue
  File "check_elasticbeanstalk__sns__sqs_eb_environment_event_notification_is_delivered_to_the_subscribed_queue.py", line 446, in run
    label, _bucket, _key = _publish_version(ctx, eb, s3, app, marker, nonce)
  File "...", line 329, in _publish_version
    assert status.upper() == "PROCESSED", (
  AssertionError: version 'v-6afb88976dae' must process cleanly before it can be deployed; DescribeApplicationVersions reports Status='UNPROCESSED'

The status stayed 'UNPROCESSED' across the check's bounded 300 s poll (1 s interval); 'PROCESSING' was never observed. The version label and the generated bucket/key are per-run examples.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite against the built emulator (region us-west-2).
2. eb.create_application(ApplicationName=app).
3. bucket = eb.create_storage_location()['S3Bucket']; s3.put_object(Bucket=bucket, Key='sim2real-eb/<app>/<label>.zip', Body=<valid zip with Procfile + server.py>).
4. eb.create_application_version(ApplicationName=app, VersionLabel=label, SourceBundle={'S3Bucket': bucket, 'S3Key': key}, Process=True).
5. Poll eb.describe_application_versions(ApplicationName=app, VersionLabels=[label])['ApplicationVersions'][0]['Status'] for 300 s: it stays 'UNPROCESSED', never becomes 'PROCESSING' and never reaches 'PROCESSED' (or 'FAILED' for a deliberately malformed bundle).
6. Re-replay elasticbeanstalk__sns__sqs.eb-environment-event-notification-is-delivered-to-the-subscribed-queue to confirm the fix; the check then proceeds to the environment-event SNS/SQS delivery assertions this stall has been masking.
