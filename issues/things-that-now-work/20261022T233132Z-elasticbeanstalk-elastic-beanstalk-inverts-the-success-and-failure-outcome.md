Issue-Source: trace-sweep-divergence
Issue-Trace: elasticbeanstalk-application-and-version-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticbeanstalk-application-and-version-lifecycle elasticbeanstalk-platform-catalog-and-account-quotas elasticbeanstalk-source-bundle-and-custom-platform-errors
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk|CreateStorageLocation|2xx->4xx|$.CreateStorageLocationResponse
Issue-Key: elasticbeanstalk|CreateStorageLocation|2xx->4xx|status
Issue-Key: elasticbeanstalk|DescribeAccountAttributes|2xx->4xx|$.DescribeAccountAttributesResponse
Issue-Key: elasticbeanstalk|DescribeAccountAttributes|2xx->4xx|status
Issue-Key: elasticbeanstalk|CreateApplicationVersion|4xx->2xx|$.ErrorResponse
Issue-Key: elasticbeanstalk|CreateApplicationVersion|4xx->2xx|status
Issue-Key: elasticbeanstalk|CreatePlatformVersion|4xx->2xx|$.ErrorResponse
Issue-Key: elasticbeanstalk|CreatePlatformVersion|4xx->2xx|status
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Elastic Beanstalk inverts the success and failure outcome of four operations

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStorageLocation

## Description
FOUR causes sharing one fault: the operation's outcome is the opposite of AWS's. Two operations AWS answers 200 are refused 400, and two requests AWS refuses 400 are accepted 200.

Cause 1 of 4: CreateStorageLocation is refused instead of answering 200.
AWS: Smithy CreateStorageLocation takes Unit input and returns 200 CreateStorageLocationResultMessage{S3Bucket (length 0..255)}; its only modeled errors are InsufficientPrivilegesException (403), S3SubscriptionRequiredException (400) and TooManyBucketsException (400) — none of which applies to a plain call. Recorded behavior: 200 with the per-account/region bucket (e.g. 'elasticbeanstalk-us-west-2-000000000000'); the call is idempotent.
Emulator: 400 — no CreateStorageLocationResponse body at all.
Why: every source-bundle workflow starts by obtaining the Beanstalk storage bucket, so a client cannot upload an application version or a custom platform bundle. The open issue '20261022T151732Z-elasticbeanstalk-elastic-beanstalk-three-modeled-operations-are-absent-from' describes the same operation being unrouted (400 InvalidAction) from a behavioral corpus; it carries no wire replay key, so this wire divergence is filed here and that issue's fix should be confirmed against this fixture step.
Fix: route and implement the operation in the elasticbeanstalk service crate (services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/create_storage_location.rs), returning a stable per-account bucket name.
Traces: elasticbeanstalk-application-and-version-lifecycle#task#5.

Cause 2 of 4: DescribeAccountAttributes is refused instead of answering 200.
AWS: Smithy DescribeAccountAttributes takes Unit input and returns 200 DescribeAccountAttributesResult{ResourceQuotas}; its single modeled error is InsufficientPrivilegesException (403). Recorded behavior: 200 with ResourceQuotas carrying ApplicationQuota, ApplicationVersionQuota, EnvironmentQuota, ConfigurationTemplateQuota, CustomPlatformQuota and HealthGroupQuota, each with a Maximum (the values themselves are not asserted).
Emulator: 400 — no DescribeAccountAttributesResponse body.
Why: a client cannot read its own service quotas, so quota-aware behavior is impossible. Same open issue as cause 1 names this operation from the behavioral corpus without a wire key.
Fix: route and implement src/ops/describe_account_attributes.rs.
Traces: elasticbeanstalk-platform-catalog-and-account-quotas#task#1.

Cause 3 of 4: CreateApplicationVersion accepts a source bundle that does not exist in S3.
AWS: Smithy CreateApplicationVersion takes SourceBundle (S3Location) and models client errors including S3LocationNotInServiceRegionException; recorded behavior: when the referenced bucket/key cannot be read, AWS answers 400 with ErrorResponse.Error {Type 'Sender', Code 'InvalidParameterCombination', Message 'Unable to download from S3 location (Bucket: <bucket>  Key: <key>). Reason: Not Found'} — the message is built from the request's bucket and key.
Emulator: 200 — the version is created as if the bundle existed, and no ErrorResponse is produced.
Why: a typo'd or missing bundle silently produces a version a client believes is deployable; the failure surfaces much later (or never), which is the opposite of AWS's fail-fast contract.
Fix: validate the SourceBundle S3 location in src/ops/create_application_version.rs and reject with HTTP 400 / Code InvalidParameterCombination.
Traces: elasticbeanstalk-source-bundle-and-custom-platform-errors#task#2.

Cause 4 of 4: CreatePlatformVersion accepts a platform definition bundle that does not exist in S3.
AWS: Smithy CreatePlatformVersion requires PlatformDefinitionBundle (S3Location) and models ElasticBeanstalkServiceException (400) among its errors; recorded behavior: an unreadable bundle is refused 400 with ErrorResponse.Error {Type 'Sender', Code 'InvalidParameterValue', Message 'Unable to download from S3 location (Bucket: <bucket>  Key: <key>). Reason: Not Found'}, again built from the request.
Emulator: 200 — the custom platform build is accepted.
Why: a client gets a platform version that was never built from a real bundle, and the modeled rejection path is unreachable. Note the open issue '20261022T151732Z-elasticbeanstalk-createplatformversion-stub' records this operation as an unimplemented scaffold; the replay shows it now answering 200, so the rejection is missing rather than stubbed.
Fix: validate PlatformDefinitionBundle in src/ops/create_platform_version.rs and reject with HTTP 400 / Code InvalidParameterValue.
Traces: elasticbeanstalk-source-bundle-and-custom-platform-errors#task#3.

## Evidence
Cause 1 — status: expected 200, got 400; $.CreateStorageLocationResponse expected {'CreateStorageLocationResult': {'S3Bucket': 'elasticbeanstalk-us-west-2-000000000000'}, 'ResponseMetadata': {...}}, got None.
Cause 2 — status: expected 200, got 400; $.DescribeAccountAttributesResponse expected {'DescribeAccountAttributesResult': {'ResourceQuotas': {'ApplicationQuota': {'Maximum': <shaped>}, 'ApplicationVersionQuota': {...}, 'EnvironmentQuota': {...}, 'ConfigurationTemplateQuota': {...}, 'CustomPlatformQuota': {...}, 'HealthGroupQuota': {...}}}}, got None.
Cause 3 — status: expected 400, got 200; $.ErrorResponse expected {'Error': {'Type': 'Sender', 'Code': 'InvalidParameterCombination', 'Message': 'Unable to download from S3 location (Bucket: sim2real-probe-elasticbeanstalk-missing-bucket-4c9bab35  Key: app.zip). Reason: Not Found'}, 'RequestId': ...}, got None.
Cause 4 — status: expected 400, got 200; $.ErrorResponse expected {'Error': {'Type': 'Sender', 'Code': 'InvalidParameterValue', 'Message': 'Unable to download from S3 location (Bucket: sim2real-probe-elasticbeanstalk-missing-bucket-4c9bab35  Key: platform.zip). Reason: Not Found'}, 'RequestId': ...}, got None.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Fixture elasticbeanstalk-application-and-version-lifecycle step task#5: CreateStorageLocation -> expect 200 with S3Bucket, observe 400.
3. Fixture elasticbeanstalk-platform-catalog-and-account-quotas step task#1: DescribeAccountAttributes -> expect 200 with ResourceQuotas, observe 400.
4. Fixture elasticbeanstalk-source-bundle-and-custom-platform-errors step task#2: CreateApplicationVersion with a SourceBundle in a non-existent bucket -> expect 400 InvalidParameterCombination, observe 200.
5. Same fixture step task#3: CreatePlatformVersion with a PlatformDefinitionBundle in a non-existent bucket -> expect 400 InvalidParameterValue, observe 200.
