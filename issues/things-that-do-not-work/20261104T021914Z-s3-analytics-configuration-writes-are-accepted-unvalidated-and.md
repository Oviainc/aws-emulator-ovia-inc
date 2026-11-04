Issue-Source: trace-sweep-divergence
Issue-Trace: s3-aaxe-configuration-id-mistakes
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-aaxe-configuration-id-mistakes s3-aaxe-data-export-schema-mistakes s3-aaxe-destination-arn-mistakes s3-aaxe-filter-shapes-beside-the-foreign-export s3-wsh-redirect-all-exclusivity-refusals-then-recover
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutBucketAnalyticsConfiguration|4xx->4xx|$.Error.Message
Issue-Key: s3|PutBucketAnalyticsConfiguration|4xx->2xx|$
Issue-Key: s3|PutBucketAnalyticsConfiguration|4xx->2xx|status
Issue-Key: s3|PutBucketAnalyticsConfiguration|4xx->4xx|$.Error.Code
Issue-Key: s3|GetBucketAnalyticsConfiguration|4xx->2xx|$.Error
Issue-Key: s3|GetBucketAnalyticsConfiguration|4xx->2xx|status
Issue-Key: s3|ListBucketAnalyticsConfigurations|2xx->2xx|$.ListBucketAnalyticsConfigurationsResult.AnalyticsConfiguration[*]
Issue-Key: s3|PutBucketWebsite|4xx->4xx|$.Error.ArgumentValue
Issue-Related: 20261102T111455Z-s3-request-ids-are-uuids-instead-of-aws.md
Issue-Commit: 490c1cbe5b0cc74feaea217ae97fc7bca73b7940

# s3 analytics configuration writes are accepted unvalidated and refusals are misworded

> _Note: this issue was identified against the code base at commit `490c1cbe5b0cc74feaea217ae97fc7bca73b7940`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketAnalyticsConfiguration

## Description
Six net-new causes on the S3 bucket-subresource configuration path. They share one fault: the analytics (and website) configuration writers do not apply S3's document-level validation, so invalid documents are stored and then served back, and the refusals that are produced carry emulator-authored codes, sentences and argument values instead of S3's. Open issues 20261030T053639Z-s3-answers-success-where-aws-refuses-the-request.md, 20261030T053639Z-s3-bucket-configuration-reads-do-not-reflect-what.md, 20261102T111455Z-s3-refusal-envelopes-omit-aws-contextual-error-members.md and 20261103T235649Z-s3-putbucketwebsite-s-over-the-limit-routing-rules.md cover neighbouring faults but the replay reports none of the clusters below as owned by them.

Cause 1 of 6: the analytics configuration-id length refusal is reworded. AWS: a PutBucketAnalyticsConfiguration whose id is outside the allowed length answers 400 with Message 'Config Id should not be more than 1 char and less than 64 chars in length' — fixed text, with no variable part (the bound is stated in the sentence, not taken from the request). Smithy: PutBucketAnalyticsConfiguration.Id is @required and targets AnalyticsId, a plain string carrying no @length trait, so the bound is AWS-side and the recording is the authority. Emulator: the same status with the re-authored sentence 'Config Id should be more than 1 character and less than 64 characters in length.' (different wording and a trailing period). Why: a client or test that matches S3's documented message to recognise an id-length rejection does not match, and the emulator's sentence additionally reverses the sense of the original. Fix: the id validation in the PutBucketAnalyticsConfiguration handler must emit AWS's exact sentence. Traces: s3-aaxe-configuration-id-mistakes#task#4.

Cause 2 of 6: a malformed analytics document is accepted. AWS: an analytics configuration body that does not validate against the published schema answers 400 Code 'MalformedXML', Message 'The XML you provided was not well-formed or did not validate against our published schema' (fixed text). Recorded behavior only (PutBucketAnalyticsConfiguration binds no modeled errors). Emulator: 204 with an empty body — the write succeeds. Why: the caller believes a configuration it cannot actually use was installed, and every later read of that bucket returns a document AWS would never have stored. Fix: schema validation of the AnalyticsConfiguration payload in the PutBucketAnalyticsConfiguration deserializer/validation step, before it reaches state. Traces: s3-aaxe-data-export-schema-mistakes#task#3, s3-aaxe-destination-arn-mistakes#task#5, s3-aaxe-filter-shapes-beside-the-foreign-export#task#7.

Cause 3 of 6: an analytics document missing StorageClassAnalysis is refused with the wrong code and sentence. AWS: 400 Code 'MissingStorageClassAnalysis', Message 'Storage Class Analysis cannot be null.' — both fixed text. Smithy: AnalyticsConfiguration.StorageClassAnalysis is @required, so its absence is a distinct, named refusal rather than a generic schema failure. Emulator: 400 Code 'MalformedXML', Message 'The XML you provided was not well-formed or did not validate against our published schema'. Why: the caller is told its XML is malformed when the document is well-formed and exactly one required element is missing, so it cannot tell which element to add; code keyed on MissingStorageClassAnalysis never fires. Fix: a required-member check in PutBucketAnalyticsConfiguration's validation that maps the missing StorageClassAnalysis onto its own error code. Traces: s3-aaxe-data-export-schema-mistakes#task#4, s3-aaxe-destination-arn-mistakes#task#2, s3-aaxe-filter-shapes-beside-the-foreign-export#task#6.

Cause 4 of 6: reading an analytics configuration that was never validly stored answers 200. AWS: GetBucketAnalyticsConfiguration for an id with no stored configuration answers 404 Code 'NoSuchConfiguration', Message 'The specified configuration does not exist.' (fixed text). Recorded behavior only (the operation binds no modeled errors). Emulator: 200 carrying the rejected document. Why: a client cannot distinguish 'configured' from 'not configured' and acts on an analytics export that does not exist on AWS. Fix: the GetBucketAnalyticsConfiguration handler must emit the NoSuchConfiguration envelope when state holds no configuration for the requested id. Traces: s3-aaxe-data-export-schema-mistakes#task#5.

Cause 5 of 6: the analytics configuration listing includes configurations AWS refused at write. AWS: ListBucketAnalyticsConfigurations returns only configurations that were accepted, so a bucket whose invalid puts were refused lists fewer AnalyticsConfiguration entries. Recorded behavior only. Emulator: 200 whose AnalyticsConfigurationList carries an extra entry for each document the emulator wrongly accepted. Why: the inventory of a bucket's analytics exports is wrong, so reconciliation and teardown loops operate on configurations that do not exist on AWS. Fix: this is the read-side consequence of cause 2 — once the writer refuses invalid documents the listing must be derived from accepted state only, in ListBucketAnalyticsConfigurations' projection. Traces: s3-aaxe-filter-shapes-beside-the-foreign-export#task#9.

Cause 6 of 6: the PutBucketWebsite exclusivity refusal echoes the request value as ArgumentValue. AWS: when a website configuration specifies RedirectAllRequestsTo together with any other property, the refusal's <ArgumentValue> is the fixed text 'not null' — it names the state the conflicting member must be in, not the value that was sent. Smithy: WebsiteConfiguration.RedirectAllRequestsTo documents 'If you specify this property, you can't specify any other property.' Emulator: the same status, code and ArgumentName, but <ArgumentValue> set to the request's redirect host. Why: a client that reads ArgumentValue to understand the constraint is shown its own input and concludes the host name is invalid, rather than that the two members are mutually exclusive. Fix: the exclusivity branch of PutBucketWebsite's validation must set ArgumentValue to the fixed 'not null'. Traces: s3-wsh-redirect-all-exclusivity-refusals-then-recover#task#2 (also #task#3, #task#4).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (score 77.8%, 705 mismatched steps).

Cause 1 — s3-aaxe-configuration-id-mistakes#task#4 (representative): Error.Message expected 'Config Id should not be more than 1 char and less than 64 chars in length', got 'Config Id should be more than 1 character and less than 64 characters in length.'

Cause 2 — s3-aaxe-data-export-schema-mistakes#task#3 (cluster also covers s3-aaxe-destination-arn-mistakes#task#5, s3-aaxe-filter-shapes-beside-the-foreign-export#task#7): status expected 400 got 204; body expected {'Error': {'Code': 'MalformedXML', 'Message': 'The XML you provided was not well-formed or did not validate against our published schema', ...}}, got an empty body.

Cause 3 — s3-aaxe-data-export-schema-mistakes#task#4 (cluster also covers s3-aaxe-destination-arn-mistakes#task#2/#3/#4/#6/#7 and s3-aaxe-filter-shapes-beside-the-foreign-export#task#6): Error.Code expected 'MissingStorageClassAnalysis' got 'MalformedXML'; Error.Message expected 'Storage Class Analysis cannot be null.' got 'The XML you provided was not well-formed or did not validate against our published schema'.

Cause 4 — s3-aaxe-data-export-schema-mistakes#task#5: status expected 404 got 200; Error expected {'Code': 'NoSuchConfiguration', 'Message': 'The specified configuration does not exist.', ...} got none.

Cause 5 — s3-aaxe-filter-shapes-beside-the-foreign-export#task#9: ListBucketAnalyticsConfigurationsResult.AnalyticsConfiguration[4] expected absent, got an extra entry (the configuration whose put AWS refused at task#7).

Cause 6 — s3-wsh-redirect-all-exclusivity-refusals-then-recover#task#2 (cluster also covers #task#3 and #task#4): Error.ArgumentValue expected 'not null', got the request's redirect host.

Per-run RequestId/HostId values also differ on several of these steps; that divergence is owned by 20261102T111455Z-s3-request-ids-are-uuids-instead-of-aws.md and is not part of this issue.

## Steps to Reproduce
1. Replay fixture s3-aaxe-configuration-id-mistakes and compare step task#4's Error.Message with the recording (cause 1).
2. Replay s3-aaxe-data-export-schema-mistakes: compare step task#3 (expected 400 MalformedXML, got 204), step task#4 (expected MissingStorageClassAnalysis) and step task#5 (expected 404 NoSuchConfiguration, got 200) — causes 2, 3 and 4.
3. Replay s3-aaxe-filter-shapes-beside-the-foreign-export and compare the AnalyticsConfiguration entries returned at step task#9 against the recording (cause 5).
4. Replay s3-wsh-redirect-all-exclusivity-refusals-then-recover and compare Error.ArgumentValue at step task#2 (cause 6).
In each case compare HTTP status, Error.Code, Error.Message and the response projection against the recorded AWS response.
