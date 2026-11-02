Issue-Source: trace-sweep-divergence
Issue-Trace: s3-put-rejects-ambiguous-overlapping-rules
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-put-rejects-ambiguous-overlapping-rules s3-put-rejects-second-prefix-rule s3-queue-and-topic-overlapping-prefixes-rejected-as-ambiguous s3-refarch-s3ev-queue-policy-gap-rejects-then-recovers s3-refarch-s3ev-topic-policy-gap-rejects-then-recovers s3-x2x-lambda-destination-reference-forms s3-x2x-lambda-destination-unpermitted-then-granted s3-x2x-overlap-and-id-rules-for-one-function s3-x2x-source-arn-forms-that-do-not-admit-the-bucket s3-refarch-s3ev-bucket-addressing-and-filter-name-mistakes s3-x2x-filter-rule-name-casing-and-generated-id s3-put-rejects-unsupported-event-type s3-x2x-deleted-function-leaves-dangling-config s3-refarch-s3ev-sequential-puts-replace-not-merge
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutBucketNotificationConfiguration|4xx->2xx|$
Issue-Key: s3|PutBucketNotificationConfiguration|4xx->2xx|status
Issue-Key: s3|GetBucketNotificationConfiguration|2xx->2xx|$.NotificationConfiguration
Issue-Key: s3|GetBucketNotificationConfiguration|2xx->2xx|$.NotificationConfiguration.QueueConfiguration[*].Filter.S3Key.FilterRule.Name
Issue-Key: s3|PutBucketNotificationConfiguration|4xx->4xx|$.Error.ArgumentName
Issue-Key: s3|PutBucketNotificationConfiguration|4xx->4xx|$.Error.ArgumentValue
Issue-Key: s3|PutBucketNotificationConfiguration|4xx->4xx|$.Error.Code
Issue-Key: s3|PutBucketNotificationConfiguration|4xx->4xx|$.Error.Message
Issue-Key: s3|PutBucketNotificationConfiguration|2xx->4xx|status
Issue-Key: s3|GetBucketNotificationConfiguration|2xx->2xx|$.NotificationConfiguration.CloudFunctionConfiguration
Issue-Key: s3|GetBucketNotificationConfiguration|2xx->2xx|$.NotificationConfiguration.CloudFunctionConfiguration.Filter.S3Key.FilterRule[*]
Issue-Key: s3|GetBucketNotificationConfiguration|2xx->2xx|$.NotificationConfiguration.CloudFunctionConfiguration.Id
Issue-Key: s3|GetBucketNotificationConfiguration|2xx->2xx|$.NotificationConfiguration.QueueConfiguration.Filter.S3Key.FilterRule.Name
Issue-Key: s3|GetBucketNotificationConfiguration|2xx->2xx|$.NotificationConfiguration.TopicConfiguration.Filter.S3Key.FilterRule.Name
Issue-Related: 20261102T111455Z-s3-refusal-envelopes-omit-aws-contextual-error-members.md
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# s3 bucket notification configuration validation and projection diverge

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Cause 1 of 7: overlapping prefix/suffix rules for the same event type are accepted. AWS: 400 Code 'InvalidArgument', Message 'Configuration is ambiguously defined. Cannot have overlapping suffixes in two rules if the prefixes are overlapping for the same event type.' (fixed text) with ArgumentName 'FilterRule'. Smithy: PutBucketNotificationConfiguration models no errors, so the rule is recorded behavior only. Emulator: 200 with an empty body - the ambiguous configuration is stored. Why: event fan-out then behaves in a way AWS would never allow, and the client never learns its configuration is ambiguous. Fix: the notification configuration validator in the Put handler. Traces: s3-put-rejects-ambiguous-overlapping-rules#task#5 (cluster of 12 steps).

Cause 2 of 7: the read after that refusal returns the stored ambiguous configuration. AWS: GetBucketNotificationConfiguration returns an empty NotificationConfiguration because the Put was refused. Recorded behavior only. Emulator: returns both QueueConfiguration entries. Fix: follows from Cause 1 - nothing must be committed. Traces: s3-put-rejects-ambiguous-overlapping-rules#task#6 (cluster of 9 steps).

Cause 3 of 7: FilterRule Name is projected lowercase. AWS: the stored rule name is serialized capitalized - 'Prefix' and 'Suffix'. Smithy: FilterRuleName's enum values are the lowercase wire tokens prefix|suffix, and the recorded AWS response capitalizes them in the GET projection, so the recording governs. Emulator: 'prefix'/'suffix'. Why: consumers that match on Name == 'Prefix' (including the AWS CLI's own output contract and CDK diffing) see no prefix rule at all. Fix: the GetBucketNotificationConfiguration serializer's FilterRule projection. Traces: s3-put-rejects-ambiguous-overlapping-rules#task#8, s3-x2x-deleted-function-leaves-dangling-config#task#6, s3-x2x-overlap-and-id-rules-for-one-function#task#7, s3-refarch-s3ev-queue-policy-gap-rejects-then-recovers#task#11, s3-refarch-s3ev-sequential-puts-replace-not-merge#task#10. (The open issue 20261102T093047Z-lambda-s3-sns-sqs-6-net-new-root names a filter-rule re-casing cause from a behavioural suite; these wire clusters are not owned by it and must be re-replayed here.)

Cause 4 of 7: an unsupported event type is refused as a document fault. AWS: 400 InvalidArgument 'The event is not supported for notifications' with ArgumentName 'Event' and ArgumentValue the event string as sent. Recorded behavior only. Emulator: 400 MalformedXML with the generic schema sentence. Why: the caller is told its XML is malformed when the document is well formed and one value is unsupported. Fix: validate Event values after parsing, not in the schema path. Traces: s3-put-rejects-unsupported-event-type#task#5.

Cause 5 of 7: the unparseable-destination refusal is re-worded. AWS: 400 InvalidArgument 'The ARN could not be parsed' (fixed text) with ArgumentName 'CloudFunction' and ArgumentValue the value as sent. Recorded behavior only. Emulator: 'The CloudFunction ARN <value> is not a valid lambda ARN' and no argument members. Fix: the destination-ARN validator. Traces: s3-x2x-lambda-destination-reference-forms#task#6.

Cause 6 of 7: a configuration AWS accepts is refused 400. AWS: 200 for these puts. Recorded behavior only. Emulator: 400. Why: valid notification wiring cannot be created at all. Fix: the same validator, which over-rejects. Traces: s3-refarch-s3ev-bucket-addressing-and-filter-name-mistakes#task#8, s3-x2x-filter-rule-name-casing-and-generated-id#task#6.

Cause 7 of 7: a Put merges/collapses rules instead of replacing the configuration, and generated ids are not AWS-shaped. AWS: the last Put wholly replaces the configuration, so the read returns the two CloudFunctionConfiguration entries ('lower' and 'upper') each with one Event; and an omitted Id is filled with an AWS-shaped generated id. Recorded behavior only. Emulator: a single entry 'both' whose Event is a two-element list, and a 32-hex generated Id. Why: a client that re-puts a configuration finds the old rules fused with the new ones, and ids do not match what AWS would mint. Fix: the notification configuration store in state (replace-not-merge) and its id generator. Traces: s3-x2x-overlap-and-id-rules-for-one-function#task#11, s3-x2x-filter-rule-name-casing-and-generated-id#task#9.

## Evidence
Replay of simparity-wire-aws-v1. Representative s3-put-rejects-ambiguous-overlapping-rules#task#5: status expected 400 got 200, empty body where AWS returns InvalidArgument 'Configuration is ambiguously defined. Cannot have overlapping suffixes in two rules if the prefixes are overlapping for the same event type.' with ArgumentName 'FilterRule' (cluster of 12). #task#6: NotificationConfiguration expected '' got two QueueConfiguration entries (cluster of 9). #task#8: FilterRule.Name expected 'Prefix' got 'prefix'. s3-put-rejects-unsupported-event-type#task#5: Code expected 'InvalidArgument' got 'MalformedXML'. s3-x2x-lambda-destination-reference-forms#task#6: Message expected 'The ARN could not be parsed'. s3-refarch-s3ev-bucket-addressing-and-filter-name-mistakes#task#8: expected 200 got 400. s3-x2x-overlap-and-id-rules-for-one-function#task#11: expected two CloudFunctionConfiguration entries, got one with a merged Event list. s3-x2x-filter-rule-name-casing-and-generated-id#task#9: CloudFunctionConfiguration.Id got '23bedc06c60e1ab162afcf5d1a08066d'.

## Steps to Reproduce
1. Replay s3-put-rejects-ambiguous-overlapping-rules and compare steps task#5, task#6, task#8. 2. Replay s3-put-rejects-unsupported-event-type step task#5 and s3-x2x-lambda-destination-reference-forms step task#6. 3. Replay s3-x2x-overlap-and-id-rules-for-one-function step task#11 and s3-x2x-filter-rule-name-casing-and-generated-id steps task#6 and task#9.
