Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail.create-trail-nonexistent-bucket-fails cloudtrail.update-event-data-store-during-active-ingestion-wrong-state-rejected cloudtrail.create-trail-bucket-without-policy-fails cloudtrail.create-trail-bucket-without-policy-rejected cloudtrail.trail-creation-on-unpolicied-bucket-rejected-and-succeeds-once-policied cloudtrail.create-trail-sns-topic-without-policy-fails cloudtrail.create-trail-unknown-kms-key-rejected cloudtrail.event-data-store-retention-period-out-of-range-rejected cloudtrail.create-trail-cloudwatch-logs-arn-validation cloudtrail.advanced-event-selectors-operators-and-field-validation cloudtrail.put-insight-selectors-rejects-insufficient-kms-policy
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail.create-trail-nonexistent-bucket-fails|_error#6|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudtrail.update-event-data-store-during-active-ingestion-wrong-state-rejected|_error#7|AssertionError|raise AssertionError(f"expected this call to be rejected, but it succeeded: {result!r}")
Issue-Key: cloudtrail.create-trail-bucket-without-policy-fails|_error#5|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: cloudtrail.create-trail-bucket-without-policy-rejected|_error#11|_Unexpected|raise _Unexpected(f"call was expected to fail but returned {str(result)[:300]!r}")
Issue-Key: cloudtrail.trail-creation-on-unpolicied-bucket-rejected-and-succeeds-once-policied|_error_code#9|AssertionError|raise AssertionError("call unexpectedly SUCCEEDED where a rejection was required")
Issue-Key: cloudtrail.create-trail-sns-topic-without-policy-fails|_error#13|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudtrail.create-trail-unknown-kms-key-rejected|_error#14|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudtrail.event-data-store-retention-period-out-of-range-rejected|_error#8|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudtrail.create-trail-cloudwatch-logs-arn-validation|_error#14|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudtrail.advanced-event-selectors-operators-and-field-validation|_error_code_and_message#15|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: cloudtrail.put-insight-selectors-rejects-insufficient-kms-policy|run#20|AssertionError|assert create_resp["KmsKeyId"] == key_arn, (
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# CloudTrail CreateTrail/UpdateTrail skip every referenced-resource and selector validation and normalize KmsKeyId away

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrail

## Description
CreateTrail and UpdateTrail in this emulator persist whatever the caller supplies. None of the modelled pre-flight checks over the S3 bucket, the SNS topic, the KMS key, the CloudWatch Logs destination or the advanced event selectors are performed, so every one of the negative classes below returns HTTP 200 with a fully-populated trail instead of the modelled 4xx refusal. There are 7 distinct causes in this issue; all 7 must be closed.

Cause 1 of 7 — CreateTrail accepts an S3 bucket that does not exist.
AWS: Smithy (com-amazonaws-cloudtrail-cloudtrail-20131101.json) binds CreateTrail error S3BucketDoesNotExistException with httpError 404 / awsQueryError code S3BucketDoesNotExist. AWS resolves S3BucketName before creating the trail and refuses when the bucket is absent.
Emulator: HTTP 200 and the trail is created. Recorded response for S3BucketName='sim2real-behav-ct-missing-bkt-...' was {'Name': ..., 'TrailARN': 'arn:aws:cloudtrail:us-west-2:000000000000:trail/...', 'LogFileValidationEnabled': False, ...} with HTTPStatusCode 200.
Why: a client that relies on CreateTrail to validate its destination silently gets a trail that can never deliver a log file; code paths that handle S3BucketDoesNotExist are never exercised.
Fix: CreateTrail's handler/validation must resolve S3BucketName against the emulator's S3 state and raise S3BucketDoesNotExistException (404).
Traces: cloudtrail.create-trail-nonexistent-bucket-fails, cloudtrail.update-event-data-store-during-active-ingestion-wrong-state-rejected.

Cause 2 of 7 — CreateTrail accepts an existing bucket that carries no CloudTrail bucket policy.
AWS: Smithy binds CreateTrail error InsufficientS3BucketPolicyException with httpError 403 / awsQueryError code InsufficientS3BucketPolicy. AWS evaluates the bucket policy for cloudtrail.amazonaws.com s3:PutObject before accepting the trail.
Emulator: HTTP 200, trail created, no policy evaluation at all. Three checks create a bucket with no policy and all three observe the create succeeding.
Why: the single most common real-world CreateTrail failure is unreachable in the emulator, so clients cannot test their remediation path, and a trail is reported healthy that AWS would never have created.
Fix: CreateTrail/UpdateTrail validation must read the target bucket's policy from S3 state and raise InsufficientS3BucketPolicyException (403) when it does not permit CloudTrail to write.
Traces: cloudtrail.create-trail-bucket-without-policy-fails, cloudtrail.create-trail-bucket-without-policy-rejected, cloudtrail.trail-creation-on-unpolicied-bucket-rejected-and-succeeds-once-policied.

Cause 3 of 7 — CreateTrail accepts an SNS topic with no CloudTrail publish policy.
AWS: Smithy binds CreateTrail error InsufficientSnsTopicPolicyException with httpError 403 / awsQueryError code InsufficientSnsTopicPolicy.
Emulator: HTTP 200 with SnsTopicARN echoed back ('arn:aws:sns:us-west-2:000000000000:sim2real-behav-ct-sns-...'), no policy check.
Why: notification wiring is accepted that AWS would refuse; the client believes notifications are configured.
Fix: CreateTrail/UpdateTrail validation must evaluate the topic policy and raise InsufficientSnsTopicPolicyException (403).
Traces: cloudtrail.create-trail-sns-topic-without-policy-fails.

Cause 4 of 7 — CreateTrail accepts a KmsKeyId naming no key.
AWS: Smithy binds CreateTrail errors KmsKeyNotFoundException (httpError 400, awsQueryError KmsKeyNotFound) and InvalidKmsKeyIdException (400). AWS resolves the key before accepting the trail.
Emulator: HTTP 200 and KmsKeyId is stored verbatim. Observed for a random key ARN and for the all-zeros key id 'arn:aws:kms:us-west-2:000000000000:key/00000000-0000-0000-0000-000000000000'.
Why: a trail is reported as SSE-KMS encrypted against a key that does not exist; the KmsKeyNotFound branch is untestable.
Fix: CreateTrail/UpdateTrail validation must resolve KmsKeyId against KMS state and raise KmsKeyNotFoundException (400).
Traces: cloudtrail.create-trail-unknown-kms-key-rejected, cloudtrail.event-data-store-retention-period-out-of-range-rejected.

Cause 5 of 7 — UpdateTrail accepts a CloudWatchLogsLogGroupArn naming no log group.
AWS: Smithy binds UpdateTrail errors InvalidCloudWatchLogsLogGroupArnException (400, awsQueryError InvalidCloudWatchLogsLogGroupArn) and CloudWatchLogsDeliveryUnavailableException (400). AWS refuses an absent log group.
Emulator: HTTP 200 and the absent group is echoed back as CloudWatchLogsLogGroupArn together with CloudWatchLogsRoleArn.
Why: CloudWatch Logs delivery is reported configured against a nonexistent destination.
Fix: UpdateTrail/CreateTrail validation must resolve the log-group ARN against CloudWatch Logs state before persisting it.
Traces: cloudtrail.create-trail-cloudwatch-logs-arn-validation.

Cause 6 of 7 — CreateTrail accepts AdvancedEventSelectors that omit the mandatory eventCategory field selector.
AWS: Smithy binds CreateTrail error InvalidEventSelectorsException is bound on PutEventSelectors/UpdateTrail (400, awsQueryError InvalidEventSelectors); recorded behavior shows AWS refusing an advanced selector set whose FieldSelectors contain only readOnly and no eventCategory.
Emulator: HTTP 200, returning AdvancedEventSelectors=[{'Name': 'sim2real-missing-event-category', 'FieldSelectors': [{'Field': 'readOnly', 'Equals': ['false']}]}] unchanged.
Why: an unusable selector set is persisted; selector validation is effectively absent, so clients cannot distinguish a valid from an invalid advanced selector.
Fix: the advanced-event-selector validator shared by CreateTrail/UpdateTrail/PutEventSelectors must require an eventCategory field selector and reject with InvalidEventSelectorsException (400).
Traces: cloudtrail.advanced-event-selectors-operators-and-field-validation.

Cause 7 of 7 — CreateTrail does not echo the fully-specified KmsKeyId it was given.
AWS: Smithy CreateTrailResponse member KmsKeyId is a String; recorded behavior shows AWS echoing the identifier in its fully-specified ARN form when a key ARN was supplied.
Emulator: given 'arn:aws:kms:us-west-2:000000000000:key/f6535e8a-d681-4d29-9b4c-d9c71673ac00' the response carried only the bare UUID 'f6535e8a-d681-4d29-9b4c-d9c71673ac00'.
Why: a client that round-trips CreateTrail output into a KMS call, or compares it against the value it supplied, gets an unusable identifier.
Fix: CreateTrail's serializer/state must keep the key identifier in the fully-specified ARN form rather than truncating it to the key id.
Traces: cloudtrail.put-insight-selectors-rejects-insufficient-kms-policy.

## Evidence
Cause 1: expected ClientError S3BucketDoesNotExistException (404); got HTTP 200 {'Name': 'sim2real-behav-ct-trail-f6567851a07c', 'S3BucketName': 'sim2real-behav-ct-missing-bkt-56f659104451', ... 'TrailARN': 'arn:aws:cloudtrail:us-west-2:000000000000:trail/sim2real-behav-ct-trail-f6567851a07c'}.
Cause 2: expected InsufficientS3BucketPolicyException (403); got HTTP 200 {'Name': 'sim2real-behav-ct-trail-d11a3e6359fe', 'S3BucketName': 'sim2real-behav-ct-bkt-7ff693416c4f', ...}. Second check raised _Unexpected: "call was expected to fail but returned ...".
Cause 3: expected InsufficientSnsTopicPolicyException (403); got HTTP 200 with 'SnsTopicARN': 'arn:aws:sns:us-west-2:000000000000:sim2real-behav-ct-sns-bd4927fb4673'.
Cause 4: expected KmsKeyNotFoundException (400); got HTTP 200 with 'KmsKeyId': 'arn:aws:kms:us-west-2:000000000000:key/920af01c-...' and, in the second trace, 'arn:aws:kms:us-west-2:000000000000:key/00000000-0000-0000-0000-000000000000'.
Cause 5: expected a ClientError on UpdateTrail with an absent log group; got HTTP 200 with 'CloudWatchLogsLogGroupArn': 'arn:aws:logs:us-west-2:000000000000:log-group:sim2real-behav-ct-lg-33f0071dc10e:*'.
Cause 6: "expected the call to be rejected, but it SUCCEEDED and returned {'TrailARN': ..., 'AdvancedEventSelectors': [{'Name': 'sim2real-missing-event-category', 'FieldSelectors': [{'Field': 'readOnly', 'Equals': ['false']}]}], 'ResponseMetadata': {'HTTPStatusCode': 200 ...}}".
Cause 7: "expected CreateTrail to echo the fully-specified key ARN 'arn:aws:kms:us-west-2:000000000000:key/f6535e8a-d681-4d29-9b4c-d9c71673ac00', got 'f6535e8a-d681-4d29-9b4c-d9c71673ac00'".

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (suite simparity-behavioral-aws-v1).
2. Replay the checks named in trace_ids.
3. Manually: (a) cloudtrail CreateTrail with an S3BucketName that was never created -> expect S3BucketDoesNotExistException 404, observe 200; (b) create a bucket with no policy and CreateTrail against it -> expect InsufficientS3BucketPolicyException 403, observe 200; (c) create an SNS topic with no policy and CreateTrail with SnsTopicName -> expect InsufficientSnsTopicPolicyException 403, observe 200; (d) CreateTrail with KmsKeyId='arn:aws:kms:<region>:<acct>:key/00000000-0000-0000-0000-000000000000' -> expect KmsKeyNotFoundException 400, observe 200; (e) UpdateTrail with CloudWatchLogsLogGroupArn naming no log group -> expect InvalidCloudWatchLogsLogGroupArnException 400, observe 200; (f) CreateTrail with AdvancedEventSelectors whose only FieldSelector is readOnly -> expect InvalidEventSelectorsException 400, observe 200; (g) CreateTrail with a full key ARN in KmsKeyId and compare the echoed KmsKeyId.
