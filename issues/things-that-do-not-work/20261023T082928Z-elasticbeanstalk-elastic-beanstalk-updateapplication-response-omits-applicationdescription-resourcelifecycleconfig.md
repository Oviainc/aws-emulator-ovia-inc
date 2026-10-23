Issue-Source: trace-sweep-divergence
Issue-Trace: elasticbeanstalk-application-and-version-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticbeanstalk-application-and-version-lifecycle elasticbeanstalk-update-application-description-mutates-in-place
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk|UpdateApplication|2xx->2xx|$.UpdateApplicationResponse.UpdateApplicationResult.Application.ResourceLifecycleConfig
Issue-Commit: 294bb54d0ff7456773c5757da3560684137bcaef

# Elastic Beanstalk UpdateApplication response omits ApplicationDescription.ResourceLifecycleConfig

> _Note: this issue was identified against the code base at commit `294bb54d0ff7456773c5757da3560684137bcaef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateApplication

## Description
ONE cause on Elastic Beanstalk's UpdateApplication response projection.

Cause 1 of 1: UpdateApplication returns an ApplicationDescription with no ResourceLifecycleConfig member.
AWS: Smithy UpdateApplication -> output ApplicationDescriptionMessage{Application: ApplicationDescription}; ApplicationDescription models ResourceLifecycleConfig (ApplicationResourceLifecycleConfig{ServiceRole, VersionLifecycleConfig}); the operation models no errors. Recorded behavior: an application created without an explicit lifecycle configuration is returned by UpdateApplication with the service-default VersionLifecycleConfig present - MaxAgeRule{Enabled false, DeleteSourceFromS3 false, MaxAgeInDays 180} and MaxCountRule{Enabled false, DeleteSourceFromS3 false, MaxCount 200}.
Emulator: HTTP 200 with $.UpdateApplicationResponse.UpdateApplicationResult.Application.ResourceLifecycleConfig absent (None). Status and every other member of the structure match.
Why: a client that updates an application and then reads the returned Application to learn or to edit its version-lifecycle policy sees no policy at all, and read-modify-write code that dereferences ResourceLifecycleConfig.VersionLifecycleConfig on the update response fails on a missing key even though the account does have a default policy.
Fix: the UpdateApplication handler's response projection of the stored application record, plus whatever defaults that record carries for the lifecycle configuration (the elasticbeanstalk service crate's update_application operation handler and its shared application state record). Open issue '20261022T233132Z-elasticbeanstalk-elastic-beanstalk-responses-omit-modeled-members-and-ignore.md' reports the same omission on the DescribeApplications and CreateApplication projections and does NOT carry this step's replay key, so the UpdateApplication response path is uncovered by it; fixing the shared record may fix both, but this response must be re-replayed on its own.
Traces: elasticbeanstalk-application-and-version-lifecycle#task#3 (the cluster also covers elasticbeanstalk-update-application-description-mutates-in-place#task#2).

## Evidence
Replay cluster service=elasticbeanstalk|UpdateApplication|status=200->200: $.UpdateApplicationResponse.UpdateApplicationResult.Application.ResourceLifecycleConfig: missing - expected {'VersionLifecycleConfig': {'MaxAgeRule': {'MaxAgeInDays': '180', 'DeleteSourceFromS3': 'false', 'Enabled': 'false'}, 'MaxCountRule': {'DeleteSourceFromS3': 'false', 'MaxCount': '200', 'Enabled': 'false'}}}, got None. Expected status 200, actual status 200 (affected_count 2).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Fixture elasticbeanstalk-application-and-version-lifecycle, step task#3 (UpdateApplication on the application created earlier in the fixture): the 200 response's Application structure has no ResourceLifecycleConfig member.
3. Same divergence at elasticbeanstalk-update-application-description-mutates-in-place#task#2.
