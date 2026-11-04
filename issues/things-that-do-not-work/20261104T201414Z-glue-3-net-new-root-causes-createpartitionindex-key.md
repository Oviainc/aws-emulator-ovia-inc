Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.create-partition-index-nonpartition-key-rejected glue.get-unfiltered-table-metadata-federation-retryable-error glue.tag-resource-invalid-arn-fails
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.create-partition-index-nonpartition-key-rejected|run#11|AssertionError|assert code == "InvalidInputException", (
Issue-Key: glue.get-unfiltered-table-metadata-federation-retryable-error|run#11|AssertionError|assert get_code == "FederationSourceRetryableException", (
Issue-Key: glue.tag-resource-invalid-arn-fails|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 15f9da03bfc4ceb53049587be90e2241d8320b45

# glue: 3 net-new root causes — CreatePartitionIndex key validation, GetUnfilteredTableMetadata federation error class, TagResource invalid-ARN message

> _Note: this issue was identified against the code base at commit `15f9da03bfc4ceb53049587be90e2241d8320b45`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePartitionIndex, GetUnfilteredTableMetadata, TagResource

## Description
Three independent net-new root causes in the glue emulator, all grounded in failing behavioral checks that are not covered by any open issue's replay key. A fixer must close ALL THREE before submitting.

Cause 1 of 3: CreatePartitionIndex does not validate that the index keys are partition columns (and is in fact still a scaffold on this path).
AWS: Smithy (com.amazonaws.glue#AWSGlue) models CreatePartitionIndex with required DatabaseName, TableName and PartitionIndex, and the client errors AlreadyExistsException / EntityNotFoundException / InvalidInputException / ResourceNumberLimitExceededException / GlueEncryptionException / OperationTimeoutException. Recorded behavior: an index whose Keys name a column that is in StorageDescriptor.Columns but NOT in the table's PartitionKeys is refused HTTP 400 InvalidInputException with message "IndexKeys not a part of PartitionColumns. Verify the indexKeys : [id]" — the fixed text is "IndexKeys not a part of PartitionColumns. Verify the indexKeys : [" + the offending key list, joined by ", ", + "]"; the names inside the brackets are the Keys taken from the request (here the single data column "id"), not part of the template. The refusal creates nothing: GetPartitionIndexes still lists no index, not even a failed one. The same call over a real partition key ("dt") is accepted and GetPartitionIndexes reports the index with Keys [{Name: "dt"}].
Emulator: the call answers NotImplemented ("operation CreatePartitionIndex is scaffolded but not implemented in this emulator", HTTP 501) instead of 400 InvalidInputException, so neither the rejection nor the accepted control index exists. Related but NOT covering: open issue 20261025T232424Z-glue-createpartitionindex-stub.md asks for the operation to be implemented and carries only the replay keys glue.partition-index-backfill-rejects-a-type-violating-partition-value and glue.partition-index-lifecycle; the check below is a third blocked unit and the IndexKeys-vs-PartitionColumns validation rule and its message are stated nowhere in it.
Why: a client that relies on Glue to reject a mis-specified partition index gets a 501 it cannot classify; retry/validation code keyed on InvalidInputException never fires, and a tool that creates indexes after table creation cannot proceed at all.
Fix: the CreatePartitionIndex handler and its input validation (reject when any PartitionIndex.Keys entry is absent from the table's PartitionKeys, before any state mutation), plus the state write that makes the accepted index visible to GetPartitionIndexes.
Traces: glue.create-partition-index-nonpartition-key-rejected.

Cause 2 of 3: GetUnfilteredTableMetadata against an accepted-but-unreachable federated database answers the wrong error class.
AWS: Smithy models GetUnfilteredTableMetadata's errors as EntityNotFoundException, FederationSourceException, FederationSourceRetryableException, GlueEncryptionException, InternalServiceException, InvalidInputException, OperationTimeoutException and PermissionTypeMismatchException. AccessDeniedException is not a modeled error of this operation at all. Recorded behavior: when CreateDatabase accepts a DatabaseInput carrying FederatedDatabase (ConnectionName pointing at a JDBC connection whose JDBC_CONNECTION_URL is unroutable), the federation source is contacted on read, and GetUnfilteredTableMetadata fails HTTP 400 with error code FederationSourceRetryableException — the retryable class that tells the caller the failure is transient in the federated source, not an authorization decision.
Emulator: the same call returns error code 'AccessDeniedException'.
Why: FederationSourceRetryableException is the one error class a client is expected to retry; returning AccessDeniedException instead makes a caller treat a transient federated-source failure as a permanent permission denial — it stops retrying, and permission-diagnosis paths fire on a condition that has nothing to do with permissions. It is also an error code outside the operation's modeled set, so a strongly typed SDK cannot map it.
Fix: the GetUnfilteredTableMetadata handler's error classification — a database whose FederatedDatabase source cannot be reached must raise FederationSourceRetryableException, and the authorization check must not be the one that fires for an unreachable federated source.
Traces: glue.get-unfiltered-table-metadata-federation-retryable-error.

Cause 3 of 3: TagResource with a malformed ARN returns an emulator-authored message where AWS returns none.
AWS: Smithy constrains TagResource.ResourceArn (GlueResourceArn) with pattern ^arn:aws(-(cn|us-gov|iso(-[bef])?))?:glue: and length 1..10240; the modeled client errors are EntityNotFoundException, InvalidInputException and OperationTimeoutException. Recorded behavior: TagResource with ResourceArn="not-a-valid-arn" is refused HTTP 400 with code InvalidInputException and an EMPTY Error.Message (the pattern violation is reported with no message text at all). TagResource over a well-formed ARN of an existing resource (arn:aws:glue:<region>:<account>:database/<name>) still succeeds and the tag is visible through GetTags, so the refusal is specific to the malformed ARN.
Emulator: the code is right but the message is authored prose: 'ResourceArn must be the ARN of a taggable Glue resource naming one of connection, crawler, database, job; got "not-a-valid-arn"'. Only the quoted ARN echoes the request; the rest of that sentence appears nowhere in AWS's response, and AWS emits no message for this error class.
Why: a client that asserts on, logs or parses Error.Message for this rejection sees text AWS never produces; message-equality assertions and log scrapers diverge, and the invented enumeration of taggable resource types implies a contract AWS does not state here.
Fix: the TagResource handler's ResourceArn validation / error serializer — raise InvalidInputException for a ResourceArn that does not match the modeled GlueResourceArn pattern, with no message body.
Traces: glue.tag-resource-invalid-arn-fails.

## Evidence
Cause 1 — glue.create-partition-index-nonpartition-key-rejected:
  expected: HTTP 400, Error.Code=InvalidInputException, Error.Message="IndexKeys not a part of PartitionColumns. Verify the indexKeys : [id]" (fixed text + the request's Keys), and GetPartitionIndexes unchanged (no index registered); then the control index over partition key "dt" accepted with Keys ["dt"].
  actual: AssertionError: CreatePartitionIndex over the data column 'id' must be refused with InvalidInputException, got NotImplemented — the handler answers HTTP 501 "operation CreatePartitionIndex is scaffolded but not implemented in this emulator".

Cause 2 — glue.get-unfiltered-table-metadata-federation-retryable-error:
  expected: GetUnfilteredTableMetadata(CatalogId=<account>, DatabaseName=<federated db>, Name=<table>, SupportedPermissionTypes=["COLUMN_PERMISSION"]) -> HTTP 400 Error.Code=FederationSourceRetryableException (a modeled error of the operation).
  actual: AssertionError: expected FederationSourceRetryableException from GetUnfilteredTableMetadata against an accepted-but-unreachable federated database, got 'AccessDeniedException' — a code that is not in the operation's modeled error set.

Cause 3 — glue.tag-resource-invalid-arn-fails:
  expected: TagResource(ResourceArn="not-a-valid-arn", TagsToAdd={"k":"v"}) -> InvalidInputException with Error.Message == "" (empty).
  actual: AssertionError: expected message '', got 'ResourceArn must be the ARN of a taggable Glue resource naming one of connection, crawler, database, job; got "not-a-valid-arn"'. The code (InvalidInputException) matches; only the message diverges.

## Steps to Reproduce
Boot the glue emulator and replay the three checks listed in trace_ids.

Cause 1: CreateDatabase; CreateTable with StorageDescriptor.Columns=[{Name:"id",Type:"bigint"}] and PartitionKeys=[{Name:"dt",Type:"string"}]; GetPartitionIndexes -> empty; CreatePartitionIndex with PartitionIndex={IndexName:<any>, Keys:["id"]} -> observe NotImplemented/501 instead of 400 InvalidInputException "IndexKeys not a part of PartitionColumns. Verify the indexKeys : [id]"; GetPartitionIndexes must still be empty; then CreatePartitionIndex with Keys:["dt"] must be accepted and listed with Keys ["dt"].

Cause 2: CreateConnection with ConnectionType="JDBC" and JDBC_CONNECTION_URL pointing at an unroutable host (e.g. jdbc:redshift://192.0.2.1:5439/dev); CreateDatabase with DatabaseInput.FederatedDatabase={ConnectionName:<that connection>, ConnectionType:"JDBC", Identifier:<any>} (accepted by the emulator); then GetUnfilteredTableMetadata(CatalogId=<account>, DatabaseName=<that db>, Name=<any table>, SupportedPermissionTypes=["COLUMN_PERMISSION"]) -> observe AccessDeniedException instead of FederationSourceRetryableException.

Cause 3: TagResource(ResourceArn="not-a-valid-arn", TagsToAdd={"k":"v"}) -> observe InvalidInputException carrying an authored message where AWS sends an empty Error.Message. Control: CreateDatabase, then TagResource over arn:aws:glue:<region>:<account>:database/<name> succeeds and GetTags shows the tag.
