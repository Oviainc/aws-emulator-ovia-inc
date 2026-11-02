Issue-Source: trace-sweep-divergence
Issue-Trace: s3-b2b-lifecycle-rule-document-mistakes
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-b2b-lifecycle-rule-document-mistakes s3-lctf-traj-tag-filter-document-mistakes s3-lcp-traj-put-lifecycle-refusals-then-recover s3-lctf-traj-tag-key-and-value-text s3-lctf-traj-expiration-period-variants
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutBucketLifecycleConfiguration|4xx->4xx|$.Error.Code
Issue-Key: s3|PutBucketLifecycleConfiguration|4xx->4xx|$.Error.Message
Issue-Key: s3|PutBucketLifecycleConfiguration|4xx->2xx|$
Issue-Key: s3|PutBucketLifecycleConfiguration|4xx->2xx|status
Issue-Key: s3|GetBucketLifecycleConfiguration|4xx->2xx|$.Error
Issue-Key: s3|GetBucketLifecycleConfiguration|4xx->2xx|status
Issue-Key: s3|GetBucketLifecycleConfiguration|2xx->2xx|$.LifecycleConfiguration.Rule[*].Expiration.Date
Issue-Related: 20261102T111455Z-s3-refusal-envelopes-omit-aws-contextual-error-members.md
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# s3 lifecycle configuration refusals and date representation diverge

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketLifecycleConfiguration

## Description
Cause 1 of 5: schema-level rule faults are reported as the emulator's own InvalidRequest prose. AWS: a lifecycle rule that mixes Date-based and Days-based actions fails document validation and answers 400 Code 'MalformedXML', Message 'The XML you provided was not well-formed or did not validate against our published schema' (fixed text, no variable part). Recorded behavior only. Emulator: 400 Code 'InvalidRequest' with the hand-written sentence 'Rule 1: mixes Date-based and Days-based actions; a rule measures every action from one of the two'. Why: callers that branch on MalformedXML to distinguish a document fault from an argument fault take the wrong branch. Fix: the PutBucketLifecycleConfiguration document validator, which must classify schema faults as MalformedXML. Traces: s3-b2b-lifecycle-rule-document-mistakes#task#4.

Cause 2 of 5: the non-negative Days refusal is re-worded and loses its argument context. AWS: 400 InvalidArgument, Message "'Days' in Transition action must be nonnegative" (fixed text), ArgumentName 'Days', ArgumentValue the value as sent. Recorded behavior only. Emulator: Message "'Days' for Transition action must be 0 or a positive integer" and no ArgumentName/ArgumentValue. Fix: the same validator's InvalidArgument construction. Traces: s3-b2b-lifecycle-rule-document-mistakes#task#6, s3-lcp-traj-put-lifecycle-refusals-then-recover#task#3.

Cause 3 of 5: the no-action refusal is re-worded. AWS: 400 InvalidArgument 'At least one action needs to be specified in a rule' (fixed text). Recorded behavior only. Emulator: 'Rule 1: names no action; a lifecycle rule carries at least one expiration or transition action'. Fix: as Cause 2. Traces: s3-lcp-traj-put-lifecycle-refusals-then-recover#task#4.

Cause 4 of 5: malformed tag-filter documents are accepted. AWS: the tag-filter documents in these steps are refused 400 MalformedXML with the fixed schema sentence. Recorded behavior only. Emulator: 200 with an empty body - the configuration is accepted. Why: the client believes a rule set is in force that AWS would never have stored, and the follow-on read then also diverges. Fix: extend the lifecycle document validator to the Filter/And/Tag shapes. Traces: s3-lctf-traj-tag-filter-document-mistakes#task#2.

Cause 5 of 5: a bucket with no lifecycle configuration reads as 200. AWS: GetBucketLifecycleConfiguration on a bucket whose Put was refused answers 404 Code 'NoSuchLifecycleConfiguration', Message 'The lifecycle configuration does not exist', with <BucketName> = the bucket addressed. Recorded behavior only. Emulator: 200 with no Error element. Why: a client cannot tell 'no policy' from 'policy present', and a refusal that should have left the bucket untouched looks like it succeeded. Fix: the GetBucketLifecycleConfiguration read path in state. Traces: s3-lctf-traj-tag-filter-document-mistakes#task#5.

Also bundled: GetBucketLifecycleConfiguration renders Expiration.Date without milliseconds. AWS returns '2030-01-01T00:00:00.000Z'; the emulator returns '2030-01-01T00:00:00Z'. Smithy: Expiration/Date is a timestamp serialized in S3's XML date-time form, which the recording shows carries three fractional digits. Fix: the lifecycle timestamp serializer. Traces: s3-lctf-traj-expiration-period-variants#task#3.

## Evidence
Replay of simparity-wire-aws-v1. Representative s3-b2b-lifecycle-rule-document-mistakes#task#4 (400/400): Code expected 'MalformedXML' got 'InvalidRequest'; Message expected 'The XML you provided was not well-formed or did not validate against our published schema' got 'Rule 1: mixes Date-based and Days-based actions; a rule measures every action from one of the two'. #task#6: Message expected "'Days' in Transition action must be nonnegative" got "'Days' for Transition action must be 0 or a positive integer", ArgumentName 'Days' and ArgumentValue '-1' missing. s3-lcp-traj-put-lifecycle-refusals-then-recover#task#4: Message expected 'At least one action needs to be specified in a rule'. s3-lctf-traj-tag-filter-document-mistakes#task#2: status expected 400 got 200, empty body. #task#5: status expected 404 got 200, expected Error NoSuchLifecycleConfiguration with BucketName. s3-lctf-traj-expiration-period-variants#task#3: Expiration.Date expected '2030-01-01T00:00:00.000Z' got '2030-01-01T00:00:00Z'.

## Steps to Reproduce
1. Replay s3-b2b-lifecycle-rule-document-mistakes and compare steps task#4 and task#6. 2. Replay s3-lcp-traj-put-lifecycle-refusals-then-recover step task#4. 3. Replay s3-lctf-traj-tag-filter-document-mistakes steps task#2 (put) and task#5 (get). 4. Replay s3-lctf-traj-expiration-period-variants step task#3 and inspect Expiration.Date.
