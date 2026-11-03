Issue-Source: trace-sweep-divergence
Issue-Trace: s3-aaxe-configuration-id-mistakes
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-aaxe-configuration-id-mistakes s3-aaxe-data-export-schema-mistakes s3-aaxe-filter-shapes-beside-the-foreign-export s3-aaxe-destination-arn-mistakes s3-aaxe-read-mistakes-on-the-per-bucket-probe
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutBucketAnalyticsConfiguration|4xx->4xx|$.Error.Code
Issue-Key: s3|PutBucketAnalyticsConfiguration|4xx->4xx|$.Error.Message
Issue-Key: s3|PutBucketAnalyticsConfiguration|4xx->2xx|$
Issue-Key: s3|PutBucketAnalyticsConfiguration|4xx->2xx|status
Issue-Key: s3|DeleteBucketAnalyticsConfiguration|4xx->2xx|$
Issue-Key: s3|DeleteBucketAnalyticsConfiguration|4xx->2xx|status
Issue-Key: s3|ListBucketAnalyticsConfigurations|2xx->4xx|$.ListBucketAnalyticsConfigurationsResult
Issue-Key: s3|ListBucketAnalyticsConfigurations|2xx->4xx|status
Issue-Key: s3|PutBucketAnalyticsConfiguration|2xx->4xx|status
Issue-Key: s3|GetBucketAnalyticsConfiguration|4xx->2xx|$.Error
Issue-Key: s3|GetBucketAnalyticsConfiguration|4xx->2xx|status
Issue-Key: s3|GetBucketAnalyticsConfiguration|2xx->4xx|$.ListBucketAnalyticsConfigurationsResult
Issue-Key: s3|GetBucketAnalyticsConfiguration|2xx->4xx|status
Issue-Key: s3|ListBucketAnalyticsConfigurations|4xx->4xx|$.Error.Code
Issue-Key: s3|ListBucketAnalyticsConfigurations|4xx->4xx|$.Error.Message
Issue-Related: 20261102T111455Z-s3-request-ids-are-uuids-instead-of-aws.md
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# s3 analytics configuration plane neither validates nor persists

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketAnalyticsConfiguration

## Description
Cause 1 of 7: an invalid analytics id is refused with an unrelated error class. AWS: PutBucketAnalyticsConfiguration with a malformed id answers 400 Code 'InvalidConfigurationId', Message 'The specified configuration id is invalid.' (fixed text). Smithy: Id is a required httpQuery member targeting AnalyticsId; the refusal class is recorded behavior only. Emulator: 400 Code 'IllegalLocationConstraintException', Message 'The unspecified location constraint is incompatible for the region specific endpoint this request was sent to.' - a bucket-creation error surfaced on a configuration write. Why: the caller is told its endpoint/region is wrong when the actual fault is the id, so no retry can succeed. Fix: the PutBucketAnalyticsConfiguration id validation in its handler (the location-constraint guard is leaking into this route). Traces: s3-aaxe-configuration-id-mistakes#task#2.

Cause 2 of 7: an id that violates the id pattern is accepted. AWS: 400 Code 'InvalidConfigId', Message 'Config Id does not match the pattern ^[a-zA-Z0-9\-_.]+$' (fixed text quoting the pattern). Recorded behavior only. Emulator: 200 with an empty body. Why: the client believes an export it can never address was created. Fix: the same validator. Traces: s3-aaxe-configuration-id-mistakes#task#3, s3-aaxe-destination-arn-mistakes#task#2.

Cause 3 of 7: DeleteBucketAnalyticsConfiguration accepts an invalid id. AWS: 400 InvalidConfigurationId 'The specified configuration id is invalid.' Recorded behavior only. Emulator: 204 No Content, i.e. a successful delete of nothing. Why: a cleanup loop reports success for an id it never deleted. Fix: share the id validator with the delete handler. Traces: s3-aaxe-configuration-id-mistakes#task#6.

Cause 4 of 7: ListBucketAnalyticsConfigurations 404s on a live bucket. AWS: 200 with ListBucketAnalyticsConfigurationsResult (IsTruncated false, plus any stored configurations). Recorded behavior only. Emulator: 404 and no result element. Why: discovery of a bucket's analytics exports is impossible. Fix: the analytics list read path in state (the bucket exists; the lookup is keyed wrongly). Traces: s3-aaxe-configuration-id-mistakes#task#7.

Cause 5 of 7: PutBucketAnalyticsConfiguration 404s on a live bucket. AWS: 204 No Content. Recorded behavior only. Emulator: 404. Why: a valid write is rejected as if the bucket did not exist. Fix: the same state lookup. Traces: s3-aaxe-configuration-id-mistakes#task#8.

Cause 6 of 7: GetBucketAnalyticsConfiguration reports an absent configuration as present. AWS: 404 Code 'NoSuchConfiguration', Message 'The specified configuration does not exist.' (fixed text). Recorded behavior only. Emulator: 200 with no Error. Why: the caller cannot distinguish 'no export configured' from 'export configured'. Fix: the analytics get handler's absence branch. Traces: s3-aaxe-data-export-schema-mistakes#task#5.

Cause 7 of 7: a stored configuration reads back 404. AWS: 200 with the stored AnalyticsConfiguration (Id, StorageClassAnalysis.DataExport with OutputSchemaVersion and Destination). Recorded behavior only. Emulator: 404 with no result. Why: write-then-read of an analytics export does not round-trip at all. Fix: the analytics store in state. Traces: s3-aaxe-read-mistakes-on-the-per-bucket-probe#task#6.

Also bundled: a fabricated continuation-token on ListBucketAnalyticsConfigurations is refused with the wrong class. AWS: 400 Code 'MalformedContinuationToken', Message 'The continuation-token you provided invalid.' (fixed text, as recorded). Emulator: 400 InvalidArgument 'The continuation token provided is incorrect'. Fix: the shared continuation-token decoder used by the bucket-configuration listings. Traces: s3-aaxe-read-mistakes-on-the-per-bucket-probe#task#4.

Note: the open issue 20261030T053639Z-s3-bucket-configuration-reads-do-not-reflect-what covers the separate projection clusters the replay marked as owned; the causes above are the net-new validation/status/persistence failures it does not carry.

## Evidence
Replay of simparity-wire-aws-v1. Representative s3-aaxe-configuration-id-mistakes#task#2 (400/400): Code expected 'InvalidConfigurationId' got 'IllegalLocationConstraintException'; Message expected 'The specified configuration id is invalid.' got 'The unspecified location constraint is incompatible for the region specific endpoint this request was sent to.' (cluster of 6 steps). #task#3: status expected 400 got 200, expected Error InvalidConfigId 'Config Id does not match the pattern ^[a-zA-Z0-9\-_.]+$' (cluster of 9 steps). #task#6: expected 400 got 204. #task#7: expected 200 got 404. #task#8: expected 204 got 404. s3-aaxe-data-export-schema-mistakes#task#5: expected 404 NoSuchConfiguration, got 200. s3-aaxe-read-mistakes-on-the-per-bucket-probe#task#6: expected 200 with the stored configuration, got 404. #task#4: Code expected 'MalformedContinuationToken' got 'InvalidArgument'.

## Steps to Reproduce
1. Replay s3-aaxe-configuration-id-mistakes and compare steps task#2, task#3, task#6, task#7 and task#8. 2. Replay s3-aaxe-data-export-schema-mistakes step task#5 (get of an absent configuration). 3. Replay s3-aaxe-read-mistakes-on-the-per-bucket-probe steps task#4 and task#6.
