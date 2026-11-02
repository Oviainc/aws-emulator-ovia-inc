Issue-Source: trace-sweep-divergence
Issue-Trace: s3-itc-put-filter-and-id-mistakes
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-itc-put-filter-and-id-mistakes s3-itc-put-tiering-day-and-enum-mistakes s3-itc-read-mistakes-on-the-per-bucket-probe
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutBucketIntelligentTieringConfiguration|4xx->2xx|$
Issue-Key: s3|PutBucketIntelligentTieringConfiguration|4xx->2xx|status
Issue-Key: s3|PutBucketIntelligentTieringConfiguration|4xx->4xx|$.Error.Code
Issue-Key: s3|PutBucketIntelligentTieringConfiguration|4xx->4xx|$.Error.Message
Issue-Key: s3|GetBucketIntelligentTieringConfiguration|4xx->2xx|$.Error
Issue-Key: s3|GetBucketIntelligentTieringConfiguration|4xx->2xx|status
Issue-Key: s3|ListBucketIntelligentTieringConfigurations|4xx->4xx|$.Error.Code
Issue-Key: s3|ListBucketIntelligentTieringConfigurations|4xx->4xx|$.Error.Message
Issue-Related: 20261102T111455Z-s3-request-ids-are-uuids-instead-of-aws.md
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# s3 intelligent-tiering validation and absence reads diverge

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketIntelligentTieringConfiguration

## Description
Cause 1 of 4: invalid intelligent-tiering documents are accepted. AWS: PutBucketIntelligentTieringConfiguration with a malformed Filter or a malformed Id answers 400 Code 'MalformedXML', Message 'The XML you provided was not well-formed or did not validate against our published schema' (fixed text). Recorded behavior only. Emulator: 200 with an empty body, so the configuration is stored. Why: the client believes a tiering policy is in force that AWS would have refused, and subsequent reads report a configuration that cannot exist on AWS. Fix: the PutBucketIntelligentTieringConfiguration document validator. Traces: s3-itc-put-filter-and-id-mistakes#task#2 (cluster of 6 steps).

Cause 2 of 4: the id/parameter mismatch is refused with emulator prose under the wrong code. AWS: 400 MalformedXML with the same fixed sentence. Recorded behavior only. Emulator: 400 Code 'InvalidArgument' with 'IntelligentTieringConfiguration.Id must be the id the request identifies ("<param id>"), got "<document id>"'. Why: callers branching on MalformedXML for document faults take the argument-fault branch. Fix: the same validator's error classification. Traces: s3-itc-put-filter-and-id-mistakes#task#6.

Cause 3 of 4: an absent configuration reads as present. AWS: GetBucketIntelligentTieringConfiguration for an id that was never stored answers 404 Code 'NoSuchConfiguration', Message 'The specified configuration does not exist.' (fixed text). Recorded behavior only. Emulator: 200 with no Error element. Why: the caller cannot tell whether its refused Put took effect. Fix: the get handler's absence branch. Traces: s3-itc-put-tiering-day-and-enum-mistakes#task#10.

Cause 4 of 4: a fabricated continuation token is misclassified. AWS: ListBucketIntelligentTieringConfigurations answers 400 Code 'MalformedContinuationToken', Message 'The continuation-token you provided invalid.' Recorded behavior only. Emulator: 400 InvalidArgument 'The continuation token provided is incorrect'. Why: a pager cannot distinguish a stale/garbled token from a bad argument. Fix: the shared continuation-token decoder used by the bucket-configuration listings. Traces: s3-itc-read-mistakes-on-the-per-bucket-probe#task#5.

## Evidence
Replay of simparity-wire-aws-v1. Representative s3-itc-put-filter-and-id-mistakes#task#2 (PutBucketIntelligentTieringConfiguration): status expected 400 got 200, body empty where AWS returns Error MalformedXML 'The XML you provided was not well-formed or did not validate against our published schema' (cluster of 6 steps). #task#6: Code expected 'MalformedXML' got 'InvalidArgument', Message got 'IntelligentTieringConfiguration.Id must be the id the request identifies ("MyIntelligentTieringConfig"), got "SomeOtherId"'. s3-itc-put-tiering-day-and-enum-mistakes#task#10: status expected 404 got 200, expected Error NoSuchConfiguration 'The specified configuration does not exist.'. s3-itc-read-mistakes-on-the-per-bucket-probe#task#5: Code expected 'MalformedContinuationToken' got 'InvalidArgument'.

## Steps to Reproduce
1. Replay s3-itc-put-filter-and-id-mistakes and compare steps task#2 and task#6. 2. Replay s3-itc-put-tiering-day-and-enum-mistakes step task#10 (get of a never-stored id). 3. Replay s3-itc-read-mistakes-on-the-per-bucket-probe step task#5 with a fabricated continuation-token.
