Issue-Source: trace-sweep-divergence
Issue-Trace: elasticbeanstalk-environment-mutation-operations-not-found-errors
Issue-Step: task#3
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticbeanstalk-environment-mutation-operations-not-found-errors elasticbeanstalk-platform-catalog-and-account-quotas
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk|TerminateEnvironment|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticbeanstalk|SwapEnvironmentCNAMEs|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticbeanstalk|DescribePlatformVersion|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 294bb54d0ff7456773c5757da3560684137bcaef

# Elastic Beanstalk not-found errors use non-AWS message templates

> _Note: this issue was identified against the code base at commit `294bb54d0ff7456773c5757da3560684137bcaef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TerminateEnvironment

## Description
THREE causes, one shared fault: when a named Elastic Beanstalk resource does not exist the emulator returns the right HTTP status and the right error code but composes the Error.Message from its own template instead of AWS's. In each case the message is BUILT FROM THE REQUEST (the environment name, the platform ARN, the caller's account id), so the template - not the recorded text - is the requirement; the recorded strings below are examples of that template applied to the fixture's values.

Cause 1 of 3: TerminateEnvironment on an unknown EnvironmentName.
AWS: Smithy TerminateEnvironment models only InsufficientPrivilegesException (403); the not-found condition is the service-common ElasticBeanstalkServiceException path. Recorded behavior: HTTP 400 with an Error.Message of the form "Environment not found with Name: <EnvironmentName> in account '<accountId>'" - i.e. the message names the requested environment NAME and the caller's account.
Emulator: HTTP 400, same error code, but Error.Message "No Environment found for EnvironmentName = '<EnvironmentName>'." - a different template that omits the account component.
Why: a client that matches on AWS's not-found wording (or parses the account out of it) to distinguish a missing environment from another 400 does not match, and the error text surfaced to operators differs from every AWS SDK/CLI transcript.
Fix: the not-found error construction in the TerminateEnvironment handler / the elasticbeanstalk crate's shared environment-lookup error builder.
Traces: elasticbeanstalk-environment-mutation-operations-not-found-errors#task#3.

Cause 2 of 3: SwapEnvironmentCNAMEs with an unknown SourceEnvironmentName.
AWS: Smithy SwapEnvironmentCNAMEs takes SourceEnvironmentName/DestinationEnvironmentName and models no operation-specific errors. Recorded behavior: HTTP 400 with Error.Message of the form "source environment not found with Name: <SourceEnvironmentName> in account '<accountId>'" - note AWS distinguishes the SOURCE role of the missing environment in the message text while still naming it by Name and account.
Emulator: HTTP 400, same code, Error.Message "No Environment found for SourceEnvironmentName = '<SourceEnvironmentName>'."
Why: same as cause 1; additionally a client cannot rely on the documented source/destination phrasing to tell which of the two inputs was bad.
Fix: the SwapEnvironmentCNAMEs handler's source/destination lookup errors.
Traces: elasticbeanstalk-environment-mutation-operations-not-found-errors#task#8.

Cause 3 of 3: DescribePlatformVersion with a PlatformArn that does not exist.
AWS: Smithy DescribePlatformVersion models ElasticBeanstalkServiceException (400, defaulted) and InsufficientPrivilegesException (403). Recorded behavior: HTTP 400 with Error.Message of the form "Platform '<PlatformArn>' does not exist." - the requested ARN quoted, then 'does not exist.'
Emulator: HTTP 400, same code, Error.Message "No Platform named '<PlatformArn>' found."
Why: same as cause 1, on the platform-catalog read path.
Fix: the DescribePlatformVersion handler's platform-lookup error construction.
Traces: elasticbeanstalk-platform-catalog-and-account-quotas#task#5.

All three statuses (400) and error codes already match; only the message templates diverge, which is why this is filed LOW. None of the open elasticbeanstalk issues carries these replay keys.

## Evidence
Cause 1 - cluster service=elasticbeanstalk|TerminateEnvironment|status=400->400: $.ErrorResponse.Error.Message value_mismatch, expected "Environment not found with Name: sim2real-probe-missing-env-b0211e0a in account '000000000000'", got "No Environment found for EnvironmentName = 'sim2real-probe-missing-env-b0211e0a'." (names are fixture-specific examples of the template).
Cause 2 - cluster service=elasticbeanstalk|SwapEnvironmentCNAMEs|status=400->400: expected "source environment not found with Name: sim2real-probe-missing-env-b0211e0a in account '000000000000'", got "No Environment found for SourceEnvironmentName = 'sim2real-probe-missing-env-b0211e0a'."
Cause 3 - cluster service=elasticbeanstalk|DescribePlatformVersion|status=400->400: expected "Platform 'arn:aws:elasticbeanstalk:us-west-2::platform/sim2real-probe-missing-platform/9.9.9' does not exist.", got "No Platform named 'arn:aws:elasticbeanstalk:us-west-2::platform/sim2real-probe-missing-platform/9.9.9' found."
In all three the expected and actual HTTP status are both 400 and no Error.Code divergence was reported.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Fixture elasticbeanstalk-environment-mutation-operations-not-found-errors, step task#3: TerminateEnvironment against a non-existent environment name -> 400 with the emulator's own message template (cause 1).
3. Same fixture, step task#8: SwapEnvironmentCNAMEs with a non-existent SourceEnvironmentName (cause 2).
4. Fixture elasticbeanstalk-platform-catalog-and-account-quotas, step task#5: DescribePlatformVersion with a non-existent platform ARN (cause 3).
