Issue-Source: trace-sweep-divergence
Issue-Trace: elasticbeanstalk-environment-mutation-operations-not-found-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticbeanstalk-environment-mutation-operations-not-found-errors elasticbeanstalk-application-tagging-and-tag-errors elasticbeanstalk-application-and-version-lifecycle
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk|CreateEnvironment|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticbeanstalk|CreateEnvironment|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticbeanstalk|UpdateTagsForResource|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticbeanstalk|UpdateTagsForResource|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticbeanstalk|ListTagsForResource|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticbeanstalk|ListTagsForResource|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticbeanstalk|UpdateApplicationResourceLifecycle|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticbeanstalk|ComposeEnvironments|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Elastic Beanstalk rejections use the wrong error code, the wrong rule, and omit RequestId

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEnvironment

## Description
FIVE causes on Elastic Beanstalk's rejection path. The HTTP status is 400 on both sides everywhere here; what diverges is the error CODE, the RULE that was applied, or the query-protocol error envelope itself.

Cause 1 of 5: CreateEnvironment reports the wrong error code because it validates in the wrong order.
AWS: Smithy CreateEnvironment takes ApplicationName (required) plus the mutually-relevant TemplateName / SolutionStackName / PlatformArn; recorded behavior: a request that names an unknown application AND supplies neither a configuration template nor a solution stack is refused 400 with Error.Code 'MissingParameter' and Message 'You must specify either Configuration Template name or Solution Stack name.' — i.e. the missing-required-input check runs before any resource lookup.
Emulator: 400 with Error.Code 'InvalidParameterValue' and a not-found message about the application.
Why: the error code is the part a client branches on. A caller that retries on MissingParameter by adding a solution stack instead sees InvalidParameterValue and concludes its application name is at fault, so the actual input defect is never corrected.
Fix: the input-validation ordering in the CreateEnvironment handler (services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/create_environment.rs) — required-parameter checks before application lookup.
Traces: elasticbeanstalk-environment-mutation-operations-not-found-errors#task#1.

Cause 2 of 5: UpdateTagsForResource's error envelope has no RequestId.
AWS: Elastic Beanstalk is an awsQuery service; its error envelope is ErrorResponse{Error{Type,Code,Message}, RequestId}, and Smithy models TooManyTagsException (awsQueryError code TooManyTagsException, HTTP 400) for this operation. Recorded behavior: exceeding the documented 50-tags-per-resource quota is refused 400 with a RequestId present in the envelope; the recorded message 'The maximum number of tags per resource is exceeded. The number of allowed tags is 50.' is an example of the quota rule (51 tags submitted, limit 50).
Emulator: 400 with $.ErrorResponse.RequestId absent (None), and a differently worded message.
Why: a missing RequestId breaks the protocol's error envelope — clients and SDK retry/logging layers read it for request correlation and some parsers require it, so every Beanstalk error becomes untraceable.
Fix: the shared awsQuery error serializer for this service (the ErrorResponse envelope must always carry RequestId).
Traces: elasticbeanstalk-application-tagging-and-tag-errors#task#6.

Cause 3 of 5: ListTagsForResource's error envelope has no RequestId either.
AWS: Smithy ListTagsForResource models ResourceNotFoundException (awsQueryError code ResourceNotFoundException, HTTP 400); recorded behavior: the 400 response carries Error{Type,Code,Message} AND RequestId.
Emulator: 400 with $.ErrorResponse.RequestId absent.
Why: same as cause 2, proving the omission is in the envelope and not in one handler. (The only other difference at this step is the quoting of the resource ARN inside the message; that is an example, not a requirement.)
Fix: same error serializer as cause 2 — this step is the second replay that must pass.
Traces: elasticbeanstalk-application-tagging-and-tag-errors#teardown#8.

Cause 4 of 5: UpdateApplicationResourceLifecycle validates ServiceRole for presence only, never for form.
AWS: Smithy UpdateApplicationResourceLifecycle requires ApplicationName and ResourceLifecycleConfig; ApplicationResourceLifecycleConfig.ServiceRole is documented as 'the ARN of an IAM service role that Elastic Beanstalk has permission to assume', required the first time a VersionLifecycleConfig is supplied. Recorded behavior: a ServiceRole that is present but is not a valid IAM role ARN is refused 400 with Message 'Parameter ServiceRole is invalid. Must be a valid IAM Role ARN.' — a validation message built from the request's member name and its constraint.
Emulator: 400 but with 'The ServiceRole property is required the first time that you provide a VersionLifecycleConfig for an application.', i.e. it reports the value as ABSENT.
Why: the two rules are different checks. A client that supplied a malformed ARN is told to supply the parameter it already supplied, so it cannot find the defect in its request; and a malformed ARN is accepted whenever the presence check passes.
Fix: add ARN-form validation for ResourceLifecycleConfig.ServiceRole in src/ops/update_application_resource_lifecycle.rs, keeping the presence check for the genuinely-absent case.
Traces: elasticbeanstalk-application-and-version-lifecycle#task#4.

Cause 5 of 5: ComposeEnvironments rejects on the wrong precondition for an unknown application.
AWS: Smithy ComposeEnvironments takes ApplicationName, GroupName and VersionLabels; recorded behavior: when the named application does not exist the request is refused 400 with Message 'Application could not be found' — the application lookup happens before any source-bundle/env.yaml inspection.
Emulator: 400 but complaining that the environment manifest (env.yaml) is not available from the version's source bundle — it skipped the application-existence check and failed on a later precondition.
Why: the reason a client is given is about a resource it never created; it will go looking for a manifest defect instead of fixing the application name, and the modeled application-not-found path is never exercised.
Fix: check application existence first in src/ops/compose_environments.rs, before manifest/version-label resolution.
Traces: elasticbeanstalk-environment-mutation-operations-not-found-errors#task#9.

Deliberately NOT filed: three clusters in this corpus differ only in the wording of a not-found message, with identical status and error code and no rule stated by the model or the docs — TerminateEnvironment (elasticbeanstalk-environment-mutation-operations-not-found-errors#task#3), SwapEnvironmentCNAMEs (#task#8) and DescribePlatformVersion (elasticbeanstalk-platform-catalog-and-account-quotas#task#5).

## Evidence
Cause 1 — $.ErrorResponse.Error.Code: expected 'MissingParameter', got 'InvalidParameterValue'; $.ErrorResponse.Error.Message: expected 'You must specify either Configuration Template name or Solution Stack name.', got "No Application named 'sim2real-probe-elasticbeanstalk-missing-app-beb4629f' found." (400 vs 400).
Cause 2 — $.ErrorResponse.RequestId: expected present (<shaped-not-asserted>), got None; message example: expected 'The maximum number of tags per resource is exceeded. The number of allowed tags is 50.', got 'The request carries 51 tags, which exceeds the limit of 50 that each resource can have.' (400 vs 400).
Cause 3 — $.ErrorResponse.RequestId: expected present (<shaped-not-asserted>), got None; message example: expected "Resource not found for ARN 'arn:aws:elasticbeanstalk:us-west-2:000000000000:application/...'.", got the same sentence without the quotes (400 vs 400).
Cause 4 — $.ErrorResponse.Error.Message: expected 'Parameter ServiceRole is invalid. Must be a valid IAM Role ARN.', got 'The ServiceRole property is required the first time that you provide a VersionLifecycleConfig for an application.' (400 vs 400) — a required-parameter rejection where AWS applies an ARN-format rejection.
Cause 5 — $.ErrorResponse.Error.Message: expected 'Application could not be found', got "The environment manifest (env.yaml) naming the environment to compose is not available from the source bundle of version label 'sim2real-probe-elasticbeanstalk-missing-app-beb4629f-v1'." (400 vs 400) — a manifest precondition where AWS applies the application-existence check.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Fixture elasticbeanstalk-environment-mutation-operations-not-found-errors step task#1: CreateEnvironment naming an unknown application and no template/solution stack -> expect Error.Code 'MissingParameter', observe 'InvalidParameterValue'.
3. Fixture elasticbeanstalk-application-tagging-and-tag-errors step task#6: UpdateTagsForResource with 51 tags -> 400 is returned but ErrorResponse has no RequestId.
4. Same fixture step teardown#8: ListTagsForResource on a deleted application -> 400 with no RequestId.
5. Fixture elasticbeanstalk-application-and-version-lifecycle step task#4: UpdateApplicationResourceLifecycle with a malformed ServiceRole -> expect the invalid-ARN rejection, observe the required-parameter rejection.
6. Fixture elasticbeanstalk-environment-mutation-operations-not-found-errors step task#9: ComposeEnvironments for an unknown application -> expect 'Application could not be found', observe the env.yaml manifest complaint.
