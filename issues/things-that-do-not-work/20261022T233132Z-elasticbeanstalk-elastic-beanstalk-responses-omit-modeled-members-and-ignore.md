Issue-Source: trace-sweep-divergence
Issue-Trace: elasticbeanstalk-application-and-version-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticbeanstalk-application-and-version-lifecycle elasticbeanstalk-create-application-arn-suffix elasticbeanstalk-create-application-basic-fields elasticbeanstalk-configuration-template-lifecycle-and-validation elasticbeanstalk-dns-availability-and-environment-discovery elasticbeanstalk-update-application-description-mutates-in-place elasticbeanstalk-environment-create-option-setting-reflected
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk|DescribeApplications|2xx->2xx|$.DescribeApplicationsResponse.DescribeApplicationsResult.Applications.member.ResourceLifecycleConfig
Issue-Key: elasticbeanstalk|CreateApplication|2xx->2xx|$.CreateApplicationResponse.CreateApplicationResult.Application.ResourceLifecycleConfig
Issue-Key: elasticbeanstalk|DescribeApplications|2xx->2xx|$.DescribeApplicationsResponse.DescribeApplicationsResult.Applications.member.Description
Issue-Key: elasticbeanstalk|CreateApplicationVersion|2xx->2xx|$.CreateApplicationVersionResponse.CreateApplicationVersionResult.ApplicationVersion.SourceBundle
Issue-Key: elasticbeanstalk|CreateApplicationVersion|2xx->2xx|$.CreateApplicationVersionResponse.CreateApplicationVersionResult.ApplicationVersion.Status
Issue-Key: elasticbeanstalk|DescribeApplicationVersions|2xx->2xx|$.DescribeApplicationVersionsResponse.DescribeApplicationVersionsResult.ApplicationVersions.member[*].SourceBundle
Issue-Key: elasticbeanstalk|DescribeApplicationVersions|2xx->2xx|$.DescribeApplicationVersionsResponse.DescribeApplicationVersionsResult.ApplicationVersions.member[*].Status
Issue-Key: elasticbeanstalk|UpdateApplicationVersion|2xx->2xx|$.UpdateApplicationVersionResponse.UpdateApplicationVersionResult.ApplicationVersion.SourceBundle
Issue-Key: elasticbeanstalk|UpdateApplicationVersion|2xx->2xx|$.UpdateApplicationVersionResponse.UpdateApplicationVersionResult.ApplicationVersion.Status
Issue-Key: elasticbeanstalk|DescribeApplicationVersions|2xx->2xx|$.DescribeApplicationVersionsResponse.DescribeApplicationVersionsResult.ApplicationVersions.member.SourceBundle
Issue-Key: elasticbeanstalk|DescribeApplicationVersions|2xx->2xx|$.DescribeApplicationVersionsResponse.DescribeApplicationVersionsResult.ApplicationVersions.member.Status
Issue-Key: elasticbeanstalk|DescribeApplications|2xx->2xx|$.DescribeApplicationsResponse.DescribeApplicationsResult.Applications
Issue-Key: elasticbeanstalk|DescribeEvents|2xx->2xx|$.DescribeEventsResponse.DescribeEventsResult.Events.member
Issue-Key: elasticbeanstalk|CreateEnvironment|2xx->2xx|$.CreateEnvironmentResponse.CreateEnvironmentResult.EnvironmentId
Issue-Key: elasticbeanstalk|CreateEnvironment|2xx->2xx|$.CreateEnvironmentResponse.CreateEnvironmentResult.PlatformArn
Issue-Key: elasticbeanstalk|CreateEnvironment|2xx->2xx|$.CreateEnvironmentResponse.CreateEnvironmentResult.Tier.Version
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Elastic Beanstalk responses omit modeled members and ignore state mutations

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeApplications

## Description
TEN causes on Elastic Beanstalk's resource records. All share one fault: the emulator's stored application / application-version / event / environment records drop members the model defines and do not carry mutations forward, so every read of them diverges while the HTTP status stays 200.

Cause 1 of 10: DescribeApplications omits ApplicationDescription.ResourceLifecycleConfig.
AWS: Smithy ApplicationDescription models ResourceLifecycleConfig (ApplicationResourceLifecycleConfig{ServiceRole, VersionLifecycleConfig}). Recorded behavior: an application created without an explicit lifecycle config is still described with the service default VersionLifecycleConfig present — MaxAgeRule{Enabled false, DeleteSourceFromS3 false, MaxAgeInDays 180} and MaxCountRule{Enabled false, DeleteSourceFromS3 false, MaxCount 200}.
Emulator: 200 with ResourceLifecycleConfig absent (None).
Why: a client reading an application's version-lifecycle policy sees no policy at all and cannot tell the defaults; code that reads ResourceLifecycleConfig.VersionLifecycleConfig before updating it fails on a missing key.
Fix: the application record's default lifecycle config in the elasticbeanstalk service crate state plus the DescribeApplications projection (services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/state.rs, src/ops/describe_applications.rs).
Traces: elasticbeanstalk-application-and-version-lifecycle#task#2 (cluster also covers elasticbeanstalk-create-application-arn-suffix#task#2 and elasticbeanstalk-create-application-basic-fields#task#2).

Cause 2 of 10: CreateApplication's own response omits the same member.
AWS: recorded behavior — CreateApplicationResult.Application carries the default ResourceLifecycleConfig described in cause 1 at creation time.
Emulator: 200 with Application.ResourceLifecycleConfig absent.
Why: the default is unobservable on the create path too, so the defect is in the record and not only in the describe projection.
Fix: src/ops/create_application.rs and the shared application record.
Traces: elasticbeanstalk-configuration-template-lifecycle-and-validation#setup#4 (cluster also covers elasticbeanstalk-create-application-arn-suffix#task#1, elasticbeanstalk-create-application-basic-fields#task#1, elasticbeanstalk-dns-availability-and-environment-discovery#task#1).

Cause 3 of 10: an application's Description update is not reflected in a later read.
AWS: recorded behavior — after UpdateApplication changes Description from 'first' to 'second', DescribeApplications returns Description 'second' (and the default ResourceLifecycleConfig).
Emulator: 200 with Description still 'first' and ResourceLifecycleConfig absent.
Why: an in-place mutation is lost, so read-after-write on an application is wrong — a client cannot confirm its own update. Related open issue '20261022T151732Z-elasticbeanstalk-updateapplication-stub' records UpdateApplication as an unimplemented scaffold; that issue carries no wire replay key, and this divergence is the DescribeApplications read it breaks.
Fix: UpdateApplication's mutation of the stored application record (src/ops/update_application.rs plus src/state.rs).
Traces: elasticbeanstalk-update-application-description-mutates-in-place#task#3.

Cause 4 of 10: CreateApplicationVersion omits SourceBundle and mis-cases Status.
AWS: Smithy ApplicationVersionDescription models SourceBundle (S3Location{S3Bucket,S3Key}) and Status (enum ApplicationVersionStatus with values Processed|Unprocessed|Failed|Processing|Building). Recorded behavior: the created version echoes SourceBundle {S3Bucket 'elasticbeanstalk-us-west-2', S3Key 'GenericSampleApplication'} and Status 'UNPROCESSED'.
Emulator: 200 with SourceBundle absent and Status 'Unprocessed'.
Why: a client cannot learn where its source bundle was recorded, and a client comparing Status to the recorded wire value fails on case.
Fix: the application-version record and its serializer (src/ops/create_application_version.rs).
Traces: elasticbeanstalk-application-and-version-lifecycle#task#6 (cluster also covers task#7).

Cause 5 of 10: DescribeApplicationVersions repeats cause 4 for every member.
AWS: recorded behavior — each ApplicationVersions member carries SourceBundle and Status 'UNPROCESSED'.
Emulator: 200, both members with SourceBundle None and Status 'Unprocessed'.
Why: the list read is as unusable as the create response for locating source bundles.
Fix: src/ops/describe_application_versions.rs and the shared record.
Traces: elasticbeanstalk-application-and-version-lifecycle#task#8.

Cause 6 of 10: UpdateApplicationVersion repeats cause 4 on its own response.
AWS: recorded behavior — UpdateApplicationVersionResult.ApplicationVersion carries SourceBundle and Status 'UNPROCESSED'.
Emulator: SourceBundle None, Status 'Unprocessed'.
Why: same as cause 4, on the update path.
Fix: src/ops/update_application_version.rs.
Traces: elasticbeanstalk-application-and-version-lifecycle#task#9.

Cause 7 of 10: DescribeApplicationVersions for a single remaining version repeats cause 4.
AWS: recorded behavior — the single member carries SourceBundle and Status 'UNPROCESSED'.
Emulator: SourceBundle None, Status 'Unprocessed'.
Why: same defect after a version delete, i.e. it is not an artifact of list shape.
Fix: as cause 5.
Traces: elasticbeanstalk-application-and-version-lifecycle#task#11.

Cause 8 of 10: DescribeApplications returns an empty Applications list where the application still exists.
AWS: recorded behavior — at this teardown step the application is still present and is described with ApplicationName, ApplicationArn, Description 'sim2real probe application updated', ConfigurationTemplates, DateCreated/DateUpdated and the default ResourceLifecycleConfig.
Emulator: 200 with Applications serialized as the empty string (no member).
Why: the application has vanished from state ahead of its deletion, so a client polling for the resource it still owns is told it does not exist. Note the summary reported state_reset_failures 0, so this is not fixture cross-talk.
Fix: the application record's lifecycle in src/state.rs and whichever preceding operation in this fixture drops it.
Traces: elasticbeanstalk-application-and-version-lifecycle#teardown#14.

Cause 9 of 10: DescribeEvents returns one event instead of the application's event list.
AWS: Smithy DescribeEvents -> EventDescriptionsMessage{Events: EventDescriptionList}. Recorded behavior: creating an application emits three INFO events sharing one RequestId — 'createApplication completed successfully.', 'Created new Application: <name>', 'createApplication is starting.' — returned newest-first as a list.
Emulator: 200 with Events.member serialized as a single structure (one event, 'createApplication completed successfully.'), not a list of three.
Why: a client reading the event stream to follow an operation sees neither the start event nor the resource-created event, and gets a scalar where it parses a list.
Fix: event emission for application operations and the DescribeEvents serializer (src/ops/describe_events.rs plus the event log in src/state.rs).
Traces: elasticbeanstalk-dns-availability-and-environment-discovery#task#4.

Cause 10 of 10: CreateEnvironment omits PlatformArn and Tier.Version and returns a non-AWS-shaped EnvironmentId.
AWS: Smithy EnvironmentDescription models PlatformArn and Tier (EnvironmentTier{Name,Type,Version}); EnvironmentId is an opaque id. Recorded behavior: creating an environment from a solution stack resolves and returns PlatformArn (the platform version ARN of the selected stack), Tier {Name 'WebServer', Type 'Standard', Version '1.0'}, and an EnvironmentId of AWS's 'e-' + 13 lowercase alphanumerics form (the recorded value itself is not asserted).
Emulator: 200 with PlatformArn absent, Tier.Version absent and EnvironmentId 'e-00000002' — a sequential counter that does not match AWS's id form.
Why: a client that passes the returned PlatformArn to a later call has nothing to pass, a client reading the worker tier version sees none, and any id-shape validation on EnvironmentId rejects the counter. (The generated id VALUE is environment-specific and not a requirement; its documented 'e-' + 13-character form is.)
Fix: solution-stack-to-platform resolution and tier defaulting in src/ops/create_environment.rs plus the environment id generator.
Traces: elasticbeanstalk-environment-create-option-setting-reflected#task#4.

## Evidence
Cause 1 — $.DescribeApplicationsResponse.DescribeApplicationsResult.Applications.member.ResourceLifecycleConfig: expected {'VersionLifecycleConfig': {'MaxAgeRule': {'MaxAgeInDays': '180', 'DeleteSourceFromS3': 'false', 'Enabled': 'false'}, 'MaxCountRule': {'DeleteSourceFromS3': 'false', 'MaxCount': '200', 'Enabled': 'false'}}}, got None (200 vs 200).
Cause 2 — $.CreateApplicationResponse.CreateApplicationResult.Application.ResourceLifecycleConfig: expected same default config, got None.
Cause 3 — $.DescribeApplicationsResponse...Applications.member.Description: expected 'second', got 'first'; .ResourceLifecycleConfig expected present, got None.
Cause 4 — $.CreateApplicationVersionResponse...ApplicationVersion.SourceBundle: expected {'S3Key': 'GenericSampleApplication', 'S3Bucket': 'elasticbeanstalk-us-west-2'}, got None; .Status expected 'UNPROCESSED', got 'Unprocessed'.
Cause 5 — same two keys on ApplicationVersions.member[0] and member[1] of DescribeApplicationVersions.
Cause 6 — same two keys on UpdateApplicationVersionResult.ApplicationVersion.
Cause 7 — same two keys on the single DescribeApplicationVersions member.
Cause 8 — $.DescribeApplicationsResponse.DescribeApplicationsResult.Applications: expected {'member': {...ApplicationName 'sim2real-probe-elasticbeanstalk-app-0e37b33e', Description 'sim2real probe application updated', ResourceLifecycleConfig {...}}}, got '' (type_mismatch).
Cause 9 — $.DescribeEventsResponse.DescribeEventsResult.Events.member: expected a 3-element list ['createApplication completed successfully.', 'Created new Application: <name>', 'createApplication is starting.'], got a single structure with only 'createApplication completed successfully.' (type_mismatch).
Cause 10 — $.CreateEnvironmentResponse.CreateEnvironmentResult.PlatformArn: expected 'arn:aws:elasticbeanstalk:us-west-2::platform/Python 3.13 running on 64bit Amazon Linux 2023/4.6.0', got None; .Tier.Version expected '1.0', got None; .EnvironmentId expected AWS's shaped id (<shaped-not-asserted>), got 'e-00000002'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Fixture elasticbeanstalk-application-and-version-lifecycle: step task#2 (DescribeApplications, no ResourceLifecycleConfig), task#6/task#8/task#9/task#11 (application-version SourceBundle absent, Status 'Unprocessed'), teardown#14 (Applications empty).
3. Fixture elasticbeanstalk-configuration-template-lifecycle-and-validation step setup#4: CreateApplication response omits ResourceLifecycleConfig.
4. Fixture elasticbeanstalk-update-application-description-mutates-in-place step task#3: Description is still 'first' after the update.
5. Fixture elasticbeanstalk-dns-availability-and-environment-discovery step task#4: DescribeEvents returns one event, not three.
6. Fixture elasticbeanstalk-environment-create-option-setting-reflected step task#4: CreateEnvironment omits PlatformArn and Tier.Version and returns EnvironmentId 'e-00000002'.
