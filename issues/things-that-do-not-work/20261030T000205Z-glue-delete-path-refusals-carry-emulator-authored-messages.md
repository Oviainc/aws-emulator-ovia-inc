Issue-Source: trace-sweep-divergence
Issue-Trace: glue-integration-property-error-paths
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-integration-property-error-paths glue-integration-resource-and-table-properties-errors glue-integration-table-properties-round-trip glue-partition-index-and-partition-column-statistics
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|DeleteIntegrationTableProperties|4xx->4xx|$.Message
Issue-Key: glue|DeleteIntegrationTableProperties|4xx->4xx|$.__type
Issue-Key: glue|DeletePartition|4xx->4xx|$.Message
Issue-Commit: 6c894fcf9edc6b5b7d1e0a3892ed526add3b28c2

# Glue delete-path refusals carry emulator-authored messages and one wrong error code (DeleteIntegrationTableProperties, DeletePartition)

> _Note: this issue was identified against the code base at commit `6c894fcf9edc6b5b7d1e0a3892ed526add3b28c2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteIntegrationTableProperties

## Description
This issue bundles 3 net-new causes on Glue DELETE refusal paths. Shared fault: when a delete request names something the emulator cannot resolve, the refusal is authored by the emulator's own generic resource/ARN checker instead of by the operation's own rule, so the message text is wrong in all three causes and the modeled exception is wrong in one. HIGH because one cause returns the wrong error code (__type), which clients branch on.

Note: the existing open issue 20261029T201859Z-glue-integration-resource-and-table-property-errors-and.md covers the Create* and Get* integration-property operations only; DeleteIntegrationTableProperties is not among its Issue-Keys, so causes 1 and 2 here are net-new. 20261029T201859Z-glue-not-found-refusals-carry-emulator-authored-messages.md likewise does not carry these replay keys.

Cause 1 of 3: DeleteIntegrationTableProperties for a target whose table properties do not exist.
AWS: Smithy: DeleteIntegrationTableProperties(ResourceArn @required @length 1..512, TableName @required @length 1..128); errors include EntityNotFoundException, ValidationException, InvalidInputException, ResourceNotFoundException. Recorded behavior: HTTP 400 and the message follows the template 'Failed to find the sourceTableProperties with sourceArn <ResourceArn> and tableName <TableName>' — fixed text 'Failed to find the sourceTableProperties with sourceArn ' + ' and tableName ', with the two variable parts taken verbatim from the request's ResourceArn and TableName members. The refusal names the missing table-properties record, not the ARN's target resource.
Emulator: HTTP 400 with the generic message template 'The target resource <ResourceArn> is not found.' — the emulator resolves the ARN's target and reports that, never naming the sourceTableProperties record or the requested TableName.
Why: a client that matches on this refusal (or logs it to tell 'the connection is gone' apart from 'no table properties were ever registered for this table') draws the wrong conclusion and goes off to re-create the connection instead of the properties; TableName, the only member that distinguishes one failure from another, is absent from the message entirely.
Fix: the DeleteIntegrationTableProperties handler's not-found refusal path (compose the message from the request's ResourceArn and TableName rather than delegating to the shared target-resource lookup message).
Traces: glue-integration-property-error-paths#task#5 (also glue-integration-resource-and-table-properties-errors#task#6).

Cause 2 of 3: DeleteIntegrationTableProperties with a ResourceArn the operation cannot interpret (e.g. a Redshift Serverless namespace ARN).
AWS: Smithy: ValidationException and InvalidInputException are both modeled on this operation. Recorded behavior: HTTP 400 with __type 'ValidationException' and the message template 'Encountered invalid resource arn <ResourceArn>' — fixed text 'Encountered invalid resource arn ' plus the request's ResourceArn. This matches the same classification real Glue uses for GetIntegrationTableProperties on an uninterpretable ARN.
Emulator: HTTP 400 with __type 'InvalidInputException' and the generic message template 'ResourceArn "<ResourceArn>" is not the ARN of a Glue connection, a Glue database or a Glue Data Catalog table'. Both the exception shape and the text come from the shared ARN-kind validator that runs ahead of the operation's own rule.
Why: the exception name is the field SDKs model as a distinct error type and clients catch by name, so a caller catching ValidationException does not catch this at all and the error escapes as an unmodeled/unknown fault; the message also asserts the ARN must be a Glue ARN, which is wrong for an operation that legitimately accepts non-Glue zero-ETL target ARNs.
Fix: DeleteIntegrationTableProperties input validation and error classification — stop pre-empting it with the generic Glue-ARN-kind check, and raise ValidationException with Glue's text.
Traces: glue-integration-table-properties-round-trip#task#2.

Cause 3 of 3: DeletePartition for a partition that does not exist on an existing table.
AWS: Smithy: DeletePartition(DatabaseName, TableName, PartitionValues all @required); errors EntityNotFoundException, InvalidInputException, OperationTimeoutException, InternalServiceException. Recorded behavior: HTTP 400 whose message is the fixed text 'Partition not found' with no variable part.
Emulator: HTTP 400 with the message 'Cannot find partition.' — emulator-authored wording (different words, different punctuation) for the same error class.
Why: Glue teardown and idempotent-delete code commonly matches this text to decide the partition was already gone and proceed; against the emulator the match fails and the cleanup path treats a benign already-deleted partition as an unexpected error.
Fix: the DeletePartition handler's not-found refusal message (serializer/handler text for EntityNotFoundException on the partition lookup).
Traces: glue-partition-index-and-partition-column-statistics#teardown#17.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false):

1. DeleteIntegrationTableProperties, status 400 expected / 400 actual, key $.Message:
   expected 'Failed to find the sourceTableProperties with sourceArn <ResourceArn> and tableName <TableName>'
   actual   'The target resource <ResourceArn> is not found.'
   (In that run ResourceArn was a connection ARN under account 000000000000 and TableName was 'orders'; the ARN, account and table name are that run's values, the fixed text and the two substituted members are the requirement.)
   Cluster: glue-integration-property-error-paths#task#5, glue-integration-resource-and-table-properties-errors#task#6.

2. DeleteIntegrationTableProperties, status 400/400, keys $.__type and $.Message:
   $.__type expected 'ValidationException', actual 'InvalidInputException'
   $.Message expected 'Encountered invalid resource arn <ResourceArn>'
   actual   'ResourceArn "<ResourceArn>" is not the ARN of a Glue connection, a Glue database or a Glue Data Catalog table'
   (The recorded ResourceArn was a redshift-serverless namespace ARN; the ARN value is that run's, the code and fixed text are the requirement.)
   Cluster: glue-integration-table-properties-round-trip#task#2.

3. DeletePartition, status 400/400, key $.Message:
   expected 'Partition not found'
   actual   'Cannot find partition.'
   Cluster: glue-partition-index-and-partition-column-statistics#teardown#17.

None of these three clusters is blocked by an unimplemented operation (stub_path empty) and none is owned by an existing open issue (owned_by empty) in the replay report.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Re-replay these steps specifically:
   - glue-integration-property-error-paths#task#5 (cause 1)
   - glue-integration-table-properties-round-trip#task#2 (cause 2)
   - glue-partition-index-and-partition-column-statistics#teardown#17 (cause 3)
4. For each, compare the response's HTTP status, $.__type and $.Message against the AWS behavior quoted per cause: cause 1 must name the sourceTableProperties with the request's ResourceArn and TableName; cause 2 must be __type ValidationException with 'Encountered invalid resource arn <ResourceArn>'; cause 3 must be exactly 'Partition not found'.
5. All three causes must be fixed before closing: a fix for one leaves the other two diverging.
