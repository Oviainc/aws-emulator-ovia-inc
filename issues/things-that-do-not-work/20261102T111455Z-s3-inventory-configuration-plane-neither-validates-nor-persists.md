Issue-Source: trace-sweep-divergence
Issue-Trace: s3-inventory-configuration-id-length-charset-and-case-rules
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-inventory-configuration-id-length-charset-and-case-rules s3-inventory-enum-casing-is-rejected-and-the-canonical-spelling-recovers s3-inventory-put-rejected-when-the-document-id-differs-from-the-parameter s3-inventory-destination-bucket-must-be-a-regionless-arn s3-inventory-destination-encryption-key-shape s3-inventory-filter-prefix-must-be-non-empty s3-inventory-delete-without-an-id-is-invalid-configuration-id s3-inventory-list-accepts-an-empty-continuation-token-but-rejects-a-fabricated-one
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutBucketInventoryConfiguration|4xx->4xx|$.Error.Code
Issue-Key: s3|PutBucketInventoryConfiguration|4xx->4xx|$.Error.Message
Issue-Key: s3|PutBucketInventoryConfiguration|4xx->2xx|$
Issue-Key: s3|PutBucketInventoryConfiguration|4xx->2xx|status
Issue-Key: s3|ListBucketInventoryConfigurations|2xx->2xx|$.ListInventoryConfigurationsResult.InventoryConfiguration[*]
Issue-Key: s3|DeleteBucketInventoryConfiguration|4xx->2xx|$
Issue-Key: s3|DeleteBucketInventoryConfiguration|4xx->2xx|status
Issue-Key: s3|ListBucketInventoryConfigurations|2xx->4xx|$.ListInventoryConfigurationsResult
Issue-Key: s3|ListBucketInventoryConfigurations|2xx->4xx|status
Issue-Key: s3|DeleteBucketInventoryConfiguration|2xx->4xx|status
Issue-Key: s3|DeleteBucketInventoryConfiguration|4xx->4xx|$.Error.Code
Issue-Key: s3|DeleteBucketInventoryConfiguration|4xx->4xx|$.Error.Message
Issue-Key: s3|ListBucketInventoryConfigurations|2xx->2xx|$.ListInventoryConfigurationsResult.InventoryConfiguration
Issue-Key: s3|ListBucketInventoryConfigurations|4xx->4xx|$.Error.Code
Issue-Key: s3|ListBucketInventoryConfigurations|4xx->4xx|$.Error.Message
Issue-Key: s3|PutBucketInventoryConfiguration|5xx->4xx|$.Error.Code
Issue-Key: s3|PutBucketInventoryConfiguration|5xx->4xx|$.Error.Message
Issue-Key: s3|PutBucketInventoryConfiguration|5xx->4xx|status
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# s3 inventory configuration plane neither validates nor persists

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketInventoryConfiguration

## Description
Cause 1 of 8: document-level inventory faults are reported as InvalidArgument with emulator prose. AWS: an inventory configuration whose id breaks the id rules fails schema validation and answers 400 Code 'MalformedXML', Message 'The XML you provided was not well-formed or did not validate against our published schema' (fixed text). Smithy: InventoryId is a plain string, so the length/charset rule and its error class are recorded behavior only. Emulator: 400 Code 'InvalidArgument' with 'Id "<id>" is not an inventory configuration id: between 1 and 64 characters'. Why: SDK/caller branching on MalformedXML misses the fault class. Fix: the PutBucketInventoryConfiguration document validator. Traces: s3-inventory-configuration-id-length-charset-and-case-rules#task#3.

Cause 2 of 8: other invalid documents are accepted outright. AWS: 400 MalformedXML with the same fixed sentence for an id with a space, a bare-name destination bucket, a regional destination ARN, a malformed SSE-KMS key and an empty filter prefix. Recorded behavior only. Emulator: 200 with an empty body. Why: the client believes an inventory report is configured that AWS would have refused. Fix: the same validator, extended to Destination and Filter. Traces: s3-inventory-configuration-id-length-charset-and-case-rules#task#4 (cluster of 5 steps).

Cause 3 of 8: the accepted-but-invalid configuration is then listed. AWS: ListBucketInventoryConfigurations returns only the configurations AWS accepted (id 'Daily.Report-v1_2'). Recorded behavior only. Emulator: the listing carries the rejected document (id 'Daily Report'). Why: the inventory inventory itself is wrong; a reconciling client sees a report that cannot exist. Fix: follows from Cause 2 - the store must never admit the document. Traces: s3-inventory-configuration-id-length-charset-and-case-rules#task#7, s3-inventory-destination-bucket-must-be-a-regionless-arn#task#5.

Cause 4 of 8: DeleteBucketInventoryConfiguration with no id succeeds. AWS: 400 Code 'InvalidConfigurationId', Message 'The specified configuration id is invalid.' Recorded behavior only. Emulator: 204 No Content. Why: a cleanup step reports a delete it never performed. Fix: the delete handler's id validation. Traces: s3-inventory-delete-without-an-id-is-invalid-configuration-id#task#4.

Cause 5 of 8: the inventory listing 404s on a live bucket. AWS: 200 with ListInventoryConfigurationsResult (the stored configuration, IsTruncated false). Recorded behavior only. Emulator: 404 with no result. Fix: the inventory list read path in state. Traces: s3-inventory-delete-without-an-id-is-invalid-configuration-id#task#5.

Cause 6 of 8: a valid delete 404s. AWS: 204 No Content. Recorded behavior only. Emulator: 404. Fix: the same state lookup. Traces: s3-inventory-delete-without-an-id-is-invalid-configuration-id#task#6.

Cause 7 of 8: deleting an unknown id blames the bucket. AWS: 404 Code 'NoSuchConfiguration', Message 'The specified configuration does not exist.' Recorded behavior only. Emulator: 404 Code 'NoSuchBucket', Message 'The specified bucket does not exist'. Why: the caller retries bucket creation for a configuration-level fault. Fix: the delete handler must distinguish a missing configuration from a missing bucket. Traces: s3-inventory-delete-without-an-id-is-invalid-configuration-id#task#7.

Cause 8 of 8: a fabricated continuation token is refused with the wrong class, and the document/parameter id mismatch answers the wrong status. AWS: 400 MalformedContinuationToken 'The continuation-token you provided invalid.'; and for an id in the document that differs from the ?id= parameter, 500 Code 'InternalError', Message 'We encountered an internal error. Please try again.' Recorded behavior only. Emulator: 400 InvalidArgument 'The continuation token provided is incorrect'; and 400 IllegalLocationConstraintException with the location-constraint sentence. Why: a pager cannot tell a stale token from a bad argument, and the mismatch case is reported as an endpoint/region fault. Fix: the shared continuation-token decoder and the inventory id-consistency check. Traces: s3-inventory-list-accepts-an-empty-continuation-token-but-rejects-a-fabricated-one#task#5, s3-inventory-put-rejected-when-the-document-id-differs-from-the-parameter#task#5.

## Evidence
Replay of simparity-wire-aws-v1. Representative s3-inventory-configuration-id-length-charset-and-case-rules#task#3 (400/400): Code expected 'MalformedXML' got 'InvalidArgument'; Message expected the fixed schema sentence, got 'Id "inv-0123...x" is not an inventory configuration id: between 1 and 64 characters'. #task#4: status expected 400 got 200 with empty body (cluster of 5). #task#7: listing carries Id 'Daily Report' where AWS carries 'Daily.Report-v1_2'. s3-inventory-delete-without-an-id-is-invalid-configuration-id#task#4: expected 400 InvalidConfigurationId, got 204; #task#5: expected 200 got 404; #task#6: expected 204 got 404; #task#7: Code expected 'NoSuchConfiguration' got 'NoSuchBucket'. s3-inventory-put-rejected-when-the-document-id-differs-from-the-parameter#task#5: status expected 500 InternalError, got 400 IllegalLocationConstraintException.

## Steps to Reproduce
1. Replay s3-inventory-configuration-id-length-charset-and-case-rules and compare steps task#3, task#4 and task#7. 2. Replay s3-inventory-delete-without-an-id-is-invalid-configuration-id steps task#4 through task#7. 3. Replay s3-inventory-list-accepts-an-empty-continuation-token-but-rejects-a-fabricated-one step task#5 and s3-inventory-put-rejected-when-the-document-id-differs-from-the-parameter step task#5.
