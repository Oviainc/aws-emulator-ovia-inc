Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.s3-analytics-xacct-only-put-validates-the-config-id-pattern s3.s3-analytics-xacct-put-rejected-for-a-config-id-over-64-characters s3.s3-analytics-xacct-put-rejected-for-a-destination-that-is-not-an-arn s3.s3-analytics-xacct-put-rejected-for-a-region-qualified-destination-arn
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.s3-analytics-xacct-only-put-validates-the-config-id-pattern|run#7|AssertionError|assert got == _NO_SUCH_CONFIG, (
Issue-Key: s3.s3-analytics-xacct-put-rejected-for-a-config-id-over-64-characters|run#7|AssertionError|assert message == _INVALID_ID_MSG, (
Issue-Key: s3.s3-analytics-xacct-put-rejected-for-a-destination-that-is-not-an-arn|run#6|AssertionError|assert got == ("InvalidDestination", "Invalid bucket ARN."), (
Issue-Key: s3.s3-analytics-xacct-put-rejected-for-a-region-qualified-destination-arn|run#7|AssertionError|assert got == _REGION_QUALIFIED_ERROR, (
Issue-Commit: 334866e49a2e1dd36c287c2dd48bc4975a5ca561

# s3 analytics-configuration refusals: 4 divergent error classes and messages (Get/Delete run Put's Id pattern check; InvalidConfigId length message reworded; destination-ARN faults answer MalformedXML instead of InvalidDestination)

> _Note: this issue was identified against the code base at commit `334866e49a2e1dd36c287c2dd48bc4975a5ca561`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketAnalyticsConfiguration

## Description
REOPENS 20261102T111455Z-s3-analytics-configuration-plane-neither-validates-nor-persists.md: its fix left the call refused. That issue claimed the analytics-configuration validation plane (PutBucketAnalyticsConfiguration / GetBucketAnalyticsConfiguration / DeleteBucketAnalyticsConfiguration, including the Id-pattern refusal and the destination-ARN traces s3-aaxe-destination-arn-mistakes) and is marked fixed; the calls below are still refused, now with the wrong error class, the wrong message, or on operations that must not validate at all. 4 net-new root causes, all in s3's analytics-configuration plane. The open issue 20261103T040901Z-s3-10-more-net-new-root-causes-bucket.md carries other analytics put refusals (empty BucketAccountId, the one-thousand-configuration cap) and does not carry any of the four checks below.

Cause 1 of 4: GetBucketAnalyticsConfiguration (and DeleteBucketAnalyticsConfiguration) run the Id pattern check that belongs only to Put.
AWS: only PutBucketAnalyticsConfiguration validates the configuration Id against the allowed pattern; it answers 400 Code 'InvalidConfigId', Message 'Config Id does not match the pattern ^[a-zA-Z0-9\-_.]+$' (fixed text that quotes the pattern; no variable parts). Get and Delete never run that check: an Id of ANY shape for which no configuration is stored is a plain not-found, 404 Code 'NoSuchConfiguration', Message 'The specified configuration does not exist.' (fixed text, no variable parts). Smithy: PutBucketAnalyticsConfiguration/GetBucketAnalyticsConfiguration/DeleteBucketAnalyticsConfiguration bind Id as a required httpQuery member targeting AnalyticsId, which is a bare string carrying no @pattern and no @length, and none of the three operations models any error - so the refusal classes are recorded behavior only.
Emulator: Put refuses the malformed Id correctly, but the same Id handed to Get on an otherwise-empty bucket answers Code 'InvalidConfigId', Message 'Config Id does not match the pattern ^[a-zA-Z0-9\-_.]+$' instead of NoSuchConfiguration/'The specified configuration does not exist.'. The validator is shared with the read/delete routes.
Why: a client that probes for a configuration (or cleans one up) by an Id it did not mint cannot tell 'no such configuration' from 'malformed Id', so absence handling and idempotent teardown both break; an InvalidConfigId on a read is not a condition a client can clear by retrying or by creating the resource.
Fix: the Id pattern validation must be invoked only by the PutBucketAnalyticsConfiguration handler; the Get and Delete handlers must go straight to the lookup and return their not-found branch.
Traces: s3.s3-analytics-xacct-only-put-validates-the-config-id-pattern.

Cause 2 of 4: the InvalidConfigId length refusal carries an emulator-authored message.
AWS: PutBucketAnalyticsConfiguration with an Id longer than 64 characters answers 400 Code 'InvalidConfigId', Message 'Config Id should not be more than 1 char and less than 64 chars in length' - fixed text in exactly that wording, with no trailing period and no variable parts; an Id of exactly 64 characters is accepted and listed. Recorded behavior only (AnalyticsId carries no @length in the model).
Emulator: the code is right and the write is correctly not stored, but the message is 'Config Id should be more than 1 character and less than 64 characters in length.' - reworded ('should be' for 'should not be more than', 'character'/'characters' for 'char'/'chars') and with a trailing period.
Why: the message is part of the refusal contract; callers and operators that match on AWS's text (log scrapers, assertion suites, support tooling) do not recognise this refusal.
Fix: the message literal in PutBucketAnalyticsConfiguration's Id validation.
Traces: s3.s3-analytics-xacct-put-rejected-for-a-config-id-over-64-characters.

Cause 3 of 4: a destination Bucket that is a bare bucket name, not an ARN, is refused MalformedXML instead of InvalidDestination.
AWS: AnalyticsS3BucketDestination.Bucket is an ARN, parsed at write time and never resolved; a bare bucket name answers 400 Code 'InvalidDestination', Message 'Invalid bucket ARN.' (fixed text, no variable parts). AWS also orders the checks: an Id outside the allowed pattern is refused InvalidConfigId before the destination is parsed, even when both fields are wrong. Smithy: Bucket is a required member of AnalyticsS3BucketDestination targeting BucketName and documented as 'The Amazon Resource Name (ARN) of the bucket to which data is exported'; the refusal class is recorded behavior only.
Emulator: 400 Code 'MalformedXML', Message 'The XML you provided was not well-formed or did not validate against our published schema' - a schema-level refusal for a document that is well-formed and schema-valid, and whose only fault is the value of one member.
Why: the caller is told its request document is malformed when the document is fine and one field's value is wrong, so no correction can be derived from the refusal; template and IaC tooling cannot surface which member to repair.
Fix: PutBucketAnalyticsConfiguration's destination validation in its handler must parse Bucket as a bucket ARN and raise InvalidDestination, after the Id check and not as a deserialization failure.
Traces: s3.s3-analytics-xacct-put-rejected-for-a-destination-that-is-not-an-arn.

Cause 4 of 4: a region-qualified destination ARN is refused MalformedXML instead of InvalidDestination.
AWS: bucket ARNs are regionless; a destination ARN that carries a region segment (the form arn:aws:s3:<region>::<bucket-name>, where the region and bucket name come from the request) answers 400 Code 'InvalidDestination', Message 'Bucket ARN should not specify a region.' (fixed text, no variable parts), while the regionless form of the same ARN is accepted and listed. Recorded behavior only.
Emulator: 400 Code 'MalformedXML', Message 'The XML you provided was not well-formed or did not validate against our published schema' - the ARN is accepted as 'well-formed enough' or rejected at the schema layer, rather than being validated as a bucket ARN.
Why: a client that region-qualifies the destination (the natural mistake, since every other S3 ARN-bearing field it touches is regional) gets a refusal that names neither the field nor the rule, and cannot distinguish it from a genuinely malformed body.
Fix: the same bucket-ARN parser used by cause 3, which must reject a non-empty region segment with its own InvalidDestination message.
Traces: s3.s3-analytics-xacct-put-rejected-for-a-region-qualified-destination-arn.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1, service heading '### s3'. All four checks are net-new: the replay marked neither an ALREADY FILED stub nor an OWNED-by issue for them.

[mismatch] s3.s3-analytics-xacct-only-put-validates-the-config-id-pattern - AssertionError: "Get never validates the pattern, so the same malformed Id must be ('NoSuchConfiguration', 'The specified configuration does not exist.'), not InvalidConfigId, on an empty bucket; got ('InvalidConfigId', 'Config Id does not match the pattern ^[a-zA-Z0-9\\-_.]+$')". The Put refusal in the same check passed, so only the Get/Delete routes are wrong.

[mismatch] s3.s3-analytics-xacct-put-rejected-for-a-config-id-over-64-characters - AssertionError: "the InvalidConfigId refusal must carry the exact message 'Config Id should not be more than 1 char and less than 64 chars in length'; got 'Config Id should be more than 1 character and less than 64 characters in length.'". Code InvalidConfigId matched.

[mismatch] s3.s3-analytics-xacct-put-rejected-for-a-destination-that-is-not-an-arn - AssertionError: "a destination Bucket that is a bare name, not an ARN, must be rejected InvalidDestination 'Invalid bucket ARN.'; got ('MalformedXML', 'The XML you provided was not well-formed or did not validate against our published schema')".

[mismatch] s3.s3-analytics-xacct-put-rejected-for-a-region-qualified-destination-arn - AssertionError: "a region-qualified destination ARN ... must be rejected ('InvalidDestination', 'Bucket ARN should not specify a region.'); got ('MalformedXML', 'The XML you provided was not well-formed or did not validate against our published schema')". (The recorded ARN in that message is a per-run generated bucket name and is not part of the requirement.)

Smithy (services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json): PutBucketAnalyticsConfiguration PUT /{Bucket}?analytics, Id = required httpQuery 'id' -> AnalyticsId (plain string, no pattern/length), AnalyticsConfiguration = required httpPayload, errors: []; GetBucketAnalyticsConfiguration GET /{Bucket}?analytics, errors: []; DeleteBucketAnalyticsConfiguration DELETE /{Bucket}?analytics (success 204), errors: []. AnalyticsS3BucketDestination.Bucket: required, doc 'The Amazon Resource Name (ARN) of the bucket to which data is exported.'

## Steps to Reproduce
1. Re-replay the four checks listed in trace_ids against the emulator.
2. Cause 1: create a bucket; call PutBucketAnalyticsConfiguration with Id 'bad id/x' and observe the correct 400 InvalidConfigId; then call GetBucketAnalyticsConfiguration (and DeleteBucketAnalyticsConfiguration) with the same Id on that otherwise-empty bucket - both must answer 404 NoSuchConfiguration 'The specified configuration does not exist.'; the emulator answers InvalidConfigId.
3. Cause 2: PutBucketAnalyticsConfiguration with a 65-character Id ('a'*65) - expect 400 InvalidConfigId with message exactly 'Config Id should not be more than 1 char and less than 64 chars in length'; then confirm a 64-character Id is accepted and listed by ListBucketAnalyticsConfigurations.
4. Cause 3: PutBucketAnalyticsConfiguration with a valid Id and StorageClassAnalysis.DataExport.Destination.S3BucketDestination.Bucket set to a bare bucket name (no 'arn:aws:s3:::' prefix) - expect 400 InvalidDestination 'Invalid bucket ARN.'; then repeat with BOTH a malformed Id and the bare-name destination and confirm the Id refusal wins.
5. Cause 4: repeat step 4 with Bucket set to 'arn:aws:s3:<region>::<bucket-name>' - expect 400 InvalidDestination 'Bucket ARN should not specify a region.'; the regionless form of the same ARN must be accepted and listed.
6. After each refusal, ListBucketAnalyticsConfigurations must still list nothing (the rejected writes store nothing).
