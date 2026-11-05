Issue-Source: trace-sweep-divergence
Issue-Trace: s3-x2x-overlap-and-id-rules-for-one-function
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-x2x-overlap-and-id-rules-for-one-function s3-cfg-bucket-tagging-refusals s3-itc-read-mistakes-on-the-per-bucket-probe s3-versioning-and-object-lock
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|GetBucketNotificationConfiguration|2xx->2xx|$.NotificationConfiguration.CloudFunctionConfiguration[*].Filter.S3Key.FilterRule[*]
Issue-Key: s3|PutBucketTagging|4xx->4xx|$.Error.Message
Issue-Key: s3|GetBucketIntelligentTieringConfiguration|4xx->4xx|$.Error.RequestId
Issue-Key: s3|DeleteBucketIntelligentTieringConfiguration|4xx->4xx|$.Error.RequestId
Issue-Key: s3|PutObjectRetention|4xx->4xx|$.Error.ArgumentValue
Issue-Commit: 7e282e985d147868e376415a13aa29a1f3675b80

# s3 response envelopes serve emulator-authored values: filter-rule casing, date rendering, request-id tokens and tag refusal wording

> _Note: this issue was identified against the code base at commit `7e282e985d147868e376415a13aa29a1f3675b80`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketNotificationConfiguration

## Description
Across four S3 response paths the emulator serves its own internal rendering of a value, or its own wording of a refusal, where AWS serves a specific wire form. Status and error code are correct in each case; what diverges is the serialized representation. 5 net-new causes, all s3.

Cause 1 of 5 — GetBucketNotificationConfiguration does not re-case FilterRule/Name
AWS: Smithy: NotificationConfiguration.LambdaFunctionConfigurations serializes with xmlName "CloudFunctionConfiguration"; FilterRule.Name targets the FilterRuleName enum whose MODEL values are lowercase ("prefix", "suffix"). Recorded behavior only for the wire form: the recorded AWS read-back serializes S3Key/FilterRule/Name as "Prefix" and "Suffix" — AWS normalises the stored lowercase enum value to its capitalised wire spelling on output, regardless of how it was written. Value is echoed from the stored configuration unchanged.
Emulator: emits Name "prefix" / "suffix" — the stored enum token verbatim, for every FilterRule of every configuration in the response.
Why: a client that matches on Name == "Prefix"/"Suffix" (as the AWS SDKs' own notification helpers and most IaC drift checks do) reads no prefix or suffix rule at all, so filtered notification configuration looks unfiltered.
Fix: the GetBucketNotificationConfiguration XML serializer — normalise FilterRuleName on output. Related open issues report the same re-casing gap from behavioral checks but do not carry this fixture step's replay key: 20261102T093047Z-lambda-s3-sns-sqs-6-net-new-root.md and 20261102T111455Z-s3-bucket-notification-configuration-validation-and-projection-diverge.md.
Traces: s3-x2x-overlap-and-id-rules-for-one-function#task#7.

Cause 2 of 5 — PutBucketTagging reserved-prefix refusal is re-worded
AWS: Smithy: PutBucketTagging.Tagging is the required httpPayload; Tag.Key targets ObjectKey (@length min 1). Recorded AWS response for a tag key in the reserved aws: namespace: HTTP 400 with Error.Message "System tags cannot be added/updated by requester" — entirely fixed text with no variable parts (it does not name the offending key).
Emulator: same status and code but Error.Message "Your TagKey cannot be prefixed with aws:" — emulator-authored wording.
Why: callers and conformance suites that assert S3's refusal text for reserved tag keys do not match, and the emulator's phrasing implies a different rule (key-prefix syntax) from AWS's (system-tag ownership).
Fix: PutBucketTagging's tag validation — use AWS's fixed message for the reserved-namespace refusal. The open issue 20261102T111455Z-s3-bucket-create-list-and-tagging-contracts-diverge.md covers neighbouring tagging contracts but not this step.
Traces: s3-cfg-bucket-tagging-refusals#task#3.

Cause 3 of 5 — GetBucketIntelligentTieringConfiguration 404 envelope carries a UUID where AWS carries a request-id token
AWS: recorded behavior only (the model binds no RequestId member; it is part of the REST-XML error envelope). The recorded response asserts Error.RequestId as an AWS request-id TOKEN — the opaque uppercase alphanumeric id S3 issues — not a value: the fixture asserts its shape, not one run's id.
Emulator: emits an RFC-4122 lowercase hyphenated UUID (e.g. a 8-4-4-4-12 form) as Error.RequestId on this operation's not-found envelope.
Why: the id's shape, not its value, is the contract; clients, log correlators and support tooling that validate or parse an S3 request id reject the UUID form. This is not a per-run value difference — the recording does not assert the value.
Fix: the shared REST-XML error-envelope serializer's request-id generator. The open issue 20261102T111455Z-s3-request-ids-are-uuids-instead-of-aws.md reports this defect class but does not carry this cluster's replay key, so the key is filed here.
Traces: s3-itc-read-mistakes-on-the-per-bucket-probe#task#3.

Cause 4 of 5 — DeleteBucketIntelligentTieringConfiguration 404 envelope: same request-id token shape
AWS: as Cause 3 — recorded behavior only; Error.RequestId is asserted as an AWS request-id token shape on the delete path's not-found envelope.
Emulator: emits a UUID.
Why: as Cause 3; recorded here because the delete path builds its own 404 envelope and must be re-replayed separately to confirm the fix.
Fix: same error-envelope serializer; see 20261102T111455Z-s3-request-ids-are-uuids-instead-of-aws.md.
Traces: s3-itc-read-mistakes-on-the-per-bucket-probe#task#4.

Cause 5 of 5 — PutObjectRetention echoes the rejected retain-until date in RFC-3339 instead of AWS's rendering
AWS: Smithy: PutObjectRetention.Retention is the httpPayload ObjectLockRetention, whose RetainUntilDate is a timestamp with timestampFormat date-time. Recorded behavior only for the refusal envelope: when the retain-until date is refused, AWS echoes it in Error.ArgumentValue using the Java Date.toString() rendering "EEE MMM dd HH:mm:ss zzz yyyy" in the service's local zone — the recorded example reads "Tue Jul 21 13:57:21 PDT 2026". The format is the requirement; the instant and zone abbreviation come from the request and the service, not from that one recording.
Emulator: echoes "2026-07-21T20:57:21.268858Z" — the same instant re-serialized as RFC-3339 with microsecond precision, i.e. the emulator's internal timestamp form.
Why: Error.ArgumentValue is documented as the value the caller supplied; a client that surfaces or re-parses it sees a representation AWS never emits, and the microsecond field is pure emulator internal detail.
Fix: PutObjectRetention's validation error construction / the error serializer's rendering of a rejected timestamp ArgumentValue.
Traces: s3-versioning-and-object-lock#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1. Per cluster, expected (recorded AWS) vs actual (emulator):

1) s3-x2x-overlap-and-id-rules-for-one-function#task#7 GetBucketNotificationConfiguration, 200 vs 200 — expected NotificationConfiguration.CloudFunctionConfiguration[0].Filter.S3Key.FilterRule[0] = {Name: "Prefix", Value: "raw/"} and [1] = {Name: "Suffix", Value: ".csv"}; actual {Name: "prefix", ...} and {Name: "suffix", ...}. Same lowercase divergence on CloudFunctionConfiguration[1] (Suffix ".CSV").
2) s3-cfg-bucket-tagging-refusals#task#3 PutBucketTagging, 400 vs 400 — expected Error.Message "System tags cannot be added/updated by requester"; actual "Your TagKey cannot be prefixed with aws:".
3) s3-itc-read-mistakes-on-the-per-bucket-probe#task#3 GetBucketIntelligentTieringConfiguration, 404 vs 404 — Error.RequestId expected as an AWS request-id token (shape asserted, value not); actual "d0bfddac-f323-4fa5-9b61-e1c63daa2056", an RFC-4122 UUID.
4) s3-itc-read-mistakes-on-the-per-bucket-probe#task#4 DeleteBucketIntelligentTieringConfiguration, 404 vs 404 — same: actual "c120761b-32f3-4f92-9966-544bb9fbc98f".
5) s3-versioning-and-object-lock#task#6 PutObjectRetention, 400 vs 400 — expected Error.ArgumentValue "Tue Jul 21 13:57:21 PDT 2026"; actual "2026-07-21T20:57:21.268858Z" (same instant, RFC-3339 with microseconds).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: put a bucket notification configuration with a Lambda target carrying S3Key filter rules, then GET /<bucket>?notification — replay s3-x2x-overlap-and-id-rules-for-one-function step task#7 and inspect FilterRule/Name casing.
3. Cause 2: PUT /<bucket>?tagging with a TagSet entry whose Key is in the aws: namespace — replay s3-cfg-bucket-tagging-refusals step task#3 and compare Error.Message.
4. Cause 3: GET /<bucket>?intelligent-tiering&id=<absent id> on a bucket with no such configuration — replay s3-itc-read-mistakes-on-the-per-bucket-probe step task#3 and inspect Error.RequestId's token shape.
5. Cause 4: DELETE the same absent configuration — replay s3-itc-read-mistakes-on-the-per-bucket-probe step task#4.
6. Cause 5: PUT /<bucket>/<key>?retention with a RetainUntilDate the service refuses — replay s3-versioning-and-object-lock step task#6 and compare Error.ArgumentValue's rendering.
