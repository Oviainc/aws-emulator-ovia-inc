Issue-Source: trace-sweep-divergence
Issue-Trace: s3-met-traj-access-point-arn-filter-is-not-a-tag-filter
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-met-traj-access-point-arn-filter-is-not-a-tag-filter s3-met-traj-config-id-rules-then-corrected s3-metrics-cfg-put-rejected-for-a-filter-carrying-both-tag-and-and s3-metrics-cfg-put-rejected-for-an-empty-filter s3-metrics-cfg-put-rejected-when-the-document-id-differs-from-the-parameter s3-met-traj-get-by-id-absent-vs-present s3-met-traj-tag-config-hides-on-the-second-page
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutBucketMetricsConfiguration|4xx->4xx|$.Error.Code
Issue-Key: s3|PutBucketMetricsConfiguration|4xx->4xx|$.Error.Message
Issue-Key: s3|PutBucketMetricsConfiguration|4xx->2xx|$
Issue-Key: s3|PutBucketMetricsConfiguration|4xx->2xx|status
Issue-Key: s3|ListBucketMetricsConfigurations|2xx->2xx|$.ListMetricsConfigurationsResult.MetricsConfiguration[*]
Issue-Key: s3|GetBucketMetricsConfiguration|2xx->4xx|$.ListMetricsConfigurationsResult
Issue-Key: s3|GetBucketMetricsConfiguration|2xx->4xx|status
Issue-Key: s3|DeleteBucketMetricsConfiguration|4xx->2xx|$
Issue-Key: s3|DeleteBucketMetricsConfiguration|4xx->2xx|status
Issue-Key: s3|DeleteBucketMetricsConfiguration|4xx->4xx|$.Error.Code
Issue-Key: s3|DeleteBucketMetricsConfiguration|4xx->4xx|$.Error.Message
Issue-Key: s3|DeleteBucketMetricsConfiguration|4xx->4xx|$.Error.RequestId
Issue-Key: s3|ListBucketMetricsConfigurations|2xx->4xx|$.ListMetricsConfigurationsResult
Issue-Key: s3|ListBucketMetricsConfigurations|2xx->4xx|status
Issue-Key: s3|ListBucketMetricsConfigurations|2xx->2xx|$.ListMetricsConfigurationsResult.NextContinuationToken
Issue-Key: s3|ListBucketMetricsConfigurations|4xx->4xx|$.Error.Code
Issue-Key: s3|ListBucketMetricsConfigurations|4xx->4xx|$.Error.Message
Issue-Key: s3|ListBucketMetricsConfigurations|4xx->4xx|$.Error.RequestId
Issue-Related: 20261102T111455Z-s3-request-ids-are-uuids-instead-of-aws.md
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# s3 metrics configuration plane neither validates nor persists

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketMetricsConfiguration

## Description
Cause 1 of 6: a MetricsFilter with more than one member is refused with the union deserializer's words. AWS: 400 Code 'MalformedXML', Message 'The XML you provided was not well-formed or did not validate against our published schema' (fixed text). Recorded behavior only. Emulator: 400 Code 'ValidationException', Message "union 'MetricsFilter' had more than one member set". Why: 'ValidationException' is not an S3 error code, so error matching fails and the message exposes the generated union decoder. Fix: validate the metrics filter in the PutBucketMetricsConfiguration handler and raise MalformedXML. Traces: s3-met-traj-access-point-arn-filter-is-not-a-tag-filter#task#4, s3-metrics-cfg-put-rejected-for-a-filter-carrying-both-tag-and-and#task#2.

Cause 2 of 6: other invalid metrics documents are accepted. AWS: 400 MalformedXML with the same fixed sentence (empty filter, document id differing from the ?id= parameter, access-point filter shapes). Recorded behavior only. Emulator: 200 with an empty body. Why: the client believes a metrics configuration is in force that AWS refused. Fix: the same validator. Traces: s3-met-traj-access-point-arn-filter-is-not-a-tag-filter#task#5.

Cause 3 of 6: the listing returns a different configuration than AWS stored. AWS: the second entry is id 'ApForeign' with Filter.AccessPointArn 'arn:aws:s3:us-east-1:<account>:accesspoint/<name>'. Recorded behavior only. Emulator: the entry is id 'ApBucketArn' with a bucket-style ARN - the rejected document was stored and the accepted one was not. Fix: follows from Cause 2 plus the metrics store ordering in state. Traces: s3-met-traj-access-point-arn-filter-is-not-a-tag-filter#task#7.

Cause 4 of 6: reads of a live metrics plane 404. AWS: GetBucketMetricsConfiguration / ListBucketMetricsConfigurations return 200 with ListMetricsConfigurationsResult carrying the three stored configurations and IsTruncated false. Recorded behavior only. Emulator: 404 with no result element. Why: discovery and per-id reads of metrics configurations are impossible after they were written. Fix: the metrics read path in state. Traces: s3-met-traj-get-by-id-absent-vs-present#task#8, s3-met-traj-get-by-id-absent-vs-present#task#11.

Cause 5 of 6: delete accepts an invalid id and blames the bucket for an unknown one. AWS: an invalid id answers 400 InvalidConfigurationId 'The specified configuration id is invalid.'; an unknown id answers 404 NoSuchConfiguration 'The specified configuration does not exist.' Recorded behavior only. Emulator: 204 No Content for the invalid id, and 404 NoSuchBucket 'The specified bucket does not exist' for the unknown one. Why: cleanup reports success it did not achieve, then misattributes the next failure to the bucket. Fix: the DeleteBucketMetricsConfiguration handler's id validation and absence branch. Traces: s3-met-traj-get-by-id-absent-vs-present#task#9, s3-met-traj-get-by-id-absent-vs-present#task#10.

Cause 6 of 6: the continuation token is a hex dump of internal state and a fabricated one is misclassified. AWS: NextContinuationToken is an opaque AWS-shaped token; a fabricated token answers 400 MalformedContinuationToken 'The continuation-token you provided invalid.' Recorded behavior only. Emulator: NextContinuationToken '73336d6331207a7a2d746167676564' (hex of an internal cursor, leaking bucket/config names) and 400 InvalidArgument 'The continuation token provided is incorrect'. Why: the token exposes internals and a pager cannot classify a stale token. Fix: the shared continuation-token encoder/decoder for bucket-configuration listings. Traces: s3-met-traj-tag-config-hides-on-the-second-page#task#103, s3-met-traj-tag-config-hides-on-the-second-page#task#105.

## Evidence
Replay of simparity-wire-aws-v1. Representative s3-met-traj-access-point-arn-filter-is-not-a-tag-filter#task#4 (400/400): Code expected 'MalformedXML' got 'ValidationException'; Message expected the fixed schema sentence, got "union 'MetricsFilter' had more than one member set" (cluster of 3 steps; also s3-metrics-cfg-put-rejected-* cluster of 3). #task#5: expected 400 got 200 empty body. #task#7: listing entry [1] expected Id 'ApForeign', got 'ApBucketArn'. s3-met-traj-get-by-id-absent-vs-present#task#8 and #task#11: expected 200 with ListMetricsConfigurationsResult, got 404. #task#9: expected 400 InvalidConfigurationId, got 204. #task#10: Code expected 'NoSuchConfiguration' got 'NoSuchBucket'. s3-met-traj-tag-config-hides-on-the-second-page#task#103: NextContinuationToken got '73336d6331207a7a2d746167676564'; #task#105: Code expected 'MalformedContinuationToken' got 'InvalidArgument'.

## Steps to Reproduce
1. Replay s3-met-traj-access-point-arn-filter-is-not-a-tag-filter and compare steps task#4, task#5 and task#7. 2. Replay s3-met-traj-get-by-id-absent-vs-present steps task#8 through task#11. 3. Replay s3-met-traj-tag-config-hides-on-the-second-page steps task#103 and task#105.
