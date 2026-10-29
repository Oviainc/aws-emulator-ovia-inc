Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.attribute-name-length-limit dynamodb.create-table-exceeds-limit-rejected dynamodb.create-table-lsi-without-range-key dynamodb.create-table-provisioned-without-throughput dynamodb.create-table-unused-attribute-definitions dynamodb.gsi-key-attribute-must-be-scalar dynamodb.gsi-multi-attribute-hash-key-query-without-every-attribute-rejected dynamodb.provisioned-table-rejects-a-switch-with-no-throughput dynamodb.update-table-rejects-a-duplicate-index-name dynamodb.put-item-on-deleting-table-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.attribute-name-length-limit|run#11|AssertionError|raise AssertionError(
Issue-Key: dynamodb.create-table-exceeds-limit-rejected|run#11|AssertionError|assert code == "LimitExceededException", f"expected LimitExceededException, got {code!r}"
Issue-Key: dynamodb.create-table-lsi-without-range-key|_error#11|AssertionError|raise AssertionError(f'expected a ClientError, but the call SUCCEEDED and returned {result!r}')
Issue-Key: dynamodb.create-table-provisioned-without-throughput|run#14|AssertionError|assert msg1 == expected1, f'expected message {expected1!r}, got {msg1!r}'
Issue-Key: dynamodb.create-table-unused-attribute-definitions|run#12|AssertionError|assert message == expected_msg, f'expected message {expected_msg!r}, got {message!r}'
Issue-Key: dynamodb.gsi-key-attribute-must-be-scalar|run#6|AssertionError|assert err.get('Message') == expected_message, (
Issue-Key: dynamodb.gsi-multi-attribute-hash-key-query-without-every-attribute-rejected|run#7|AssertionError|assert msg == (
Issue-Key: dynamodb.provisioned-table-rejects-a-switch-with-no-throughput|run#9|AssertionError|assert err.get('Message') == expected, f"expected message {expected!r}, got {err.get('Message')!r}"
Issue-Key: dynamodb.update-table-rejects-a-duplicate-index-name|run#10|AssertionError|assert err.get('Message') == expected, f"expected message {expected!r}, got {err.get('Message')!r}"
Issue-Key: dynamodb.put-item-on-deleting-table-rejected|_error#14|AssertionError|raise AssertionError("expected the call to be refused, but it succeeded")
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# DynamoDB CreateTable/UpdateTable/PutItem validation: 10 missing or wrongly-worded refusals

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
Ten net-new root causes in the DynamoDB control-plane/write-path validation. All are in the `dynamodb` service and each needs its own fix before this issue can be closed.

Cause 1 of 10: PutItem accepts an over-length attribute name.
AWS: an item attribute name may be at most 65535 bytes; at 65536 bytes PutItem is refused with `ValidationException` (HTTP 400) and the message `1 validation error detected: Attribute name is too large, must be less than 65536 bytes` (fixed text; no variable part). Smithy: PutItem.Item -> PutItemInputAttributeMap keyed by AttributeName; service-common error ValidationException.
Emulator: PutItem with a 65536-byte attribute name SUCCEEDS (200) and the item is stored.
Why: a client that relies on the service to reject oversized names silently persists items real AWS refuses, and its error path is never exercised.
Fix: PutItem validation (attribute-name length check before the write reaches state).
Traces: dynamodb.attribute-name-length-limit

Cause 2 of 10: CreateTable over the table-count limit returns the wrong error code.
AWS: exceeding the per-account table limit is refused with `LimitExceededException` (HTTP 400), which CreateTable models explicitly (Smithy: CreateTable errors include LimitExceededException).
Emulator: returns `ValidationException` instead.
Why: callers that catch LimitExceededException to back off/raise a quota request never see it; the call looks like a malformed request instead of a quota condition.
Fix: CreateTable handler's limit check (error classification).
Traces: dynamodb.create-table-exceeds-limit-rejected

Cause 3 of 10: CreateTable accepts an LSI on a table whose key schema has no RANGE key.
AWS: a LocalSecondaryIndex requires the base table to have a composite (HASH+RANGE) key; declaring one over a HASH-only table is refused with `ValidationException` (HTTP 400).
Emulator: the call SUCCEEDS and returns a TableDescription with TableStatus CREATING and the LSI present (recorded response: LocalSecondaryIndexes carrying index `lsi1` over a KeySchema with only a HASH element).
Why: an invalid table is created; subsequent LSI queries behave in ways no real account can reproduce, and the negative path is untested.
Fix: CreateTable validation (LSI/base key-schema consistency).
Traces: dynamodb.create-table-lsi-without-range-key

Cause 4 of 10: CreateTable PROVISIONED-without-throughput message is reworded.
AWS: message is `One or more parameter values were invalid: ReadCapacityUnits and WriteCapacityUnits must both be specified when BillingMode is PROVISIONED` — entirely fixed text.
Emulator: `One or more parameter values were invalid: Both ReadCapacityUnits and WriteCapacityUnits must be specified when BillingMode is PROVISIONED`.
Why: clients and test suites that match the service's wording (and humans reading logs) see text AWS never emits.
Fix: CreateTable validation message text.
Traces: dynamodb.create-table-provisioned-without-throughput

Cause 5 of 10: CreateTable unused-AttributeDefinitions message is an emulator invention.
AWS: `One or more parameter values were invalid: Number of attributes in KeySchema does not exactly match number of attributes defined in AttributeDefinitions` — fixed text, no variable part.
Emulator: `Some AttributeDefinitions are not used in any KeySchema: never_used` (invents a message and appends the offending attribute name).
Why: the error class is unrecognisable to clients keyed on AWS's wording and leaks an implementation-specific explanation.
Fix: CreateTable validation message text.
Traces: dynamodb.create-table-unused-attribute-definitions

Cause 6 of 10: constraint-violation message lists the AttributeType enum in the wrong order.
AWS: `1 validation error detected: Value '<supplied value>' at 'attributeDefinitions.<n>.member.attributeType' failed to satisfy constraint: Member must satisfy enum value set: [B, N, S]` — the enum set is rendered in AWS's order `[B, N, S]`; the value and index come from the request.
Emulator: renders `[S, N, B]`.
Why: the message differs from AWS's for every enum-constraint violation on this member, so exact-message assertions and log fingerprints diverge.
Fix: the shared enum-constraint message serializer used by CreateTable validation.
Traces: dynamodb.gsi-key-attribute-must-be-scalar

Cause 7 of 10: multi-HASH GSI key schema is refused with a message about the wrong element.
AWS: a GSI key schema may have at most one HASH and one RANGE element; extra elements are refused with `ValidationException` naming the invalid key schema for that index.
Emulator: message is `Invalid KeySchema for index five-hash: The fifth KeySchemaElement is not a RANGE key type` — it describes an ordinal position AWS does not name (the fixed part should state the key-schema rule; the only variable part is the index name taken from the request).
Why: the refusal mis-describes the defect, so a caller cannot act on it.
Fix: CreateTable/UpdateTable GSI key-schema validation message.
Traces: dynamodb.gsi-multi-attribute-hash-key-query-without-every-attribute-rejected

Cause 8 of 10: UpdateTable switch to PROVISIONED with no throughput uses the CreateTable message.
AWS: `One or more parameter values were invalid: ProvisionedThroughput must be specified when BillingMode is PROVISIONED` (fixed text) on UpdateTable.
Emulator: emits the CreateTable-shaped `One or more parameter values were invalid: Both ReadCapacityUnits and WriteCapacityUnits must be specified when BillingMode is PROVISIONED`.
Why: the two operations have different messages in AWS; collapsing them hides which parameter the caller actually has to supply.
Fix: UpdateTable validation message text.
Traces: dynamodb.provisioned-table-rejects-a-switch-with-no-throughput

Cause 9 of 10: UpdateTable duplicate-index-name message is reworded and leaks state.
AWS: `Attempting to create an index which already exists` — fixed text with no variable part.
Emulator: `Invalid GlobalSecondaryIndexUpdates: index <index name> already exists on table <table name>`, interpolating request/state values AWS does not include.
Why: the refusal does not match AWS's wording for the error class.
Fix: UpdateTable GlobalSecondaryIndexUpdates validation message.
Traces: dynamodb.update-table-rejects-a-duplicate-index-name

Cause 10 of 10: PutItem against a table in DELETING state succeeds.
AWS: a data-plane write to a table that is not ACTIVE is refused (`ResourceNotFoundException`/`ValidationException` per the recording's expectation that the call be refused).
Emulator: the PutItem SUCCEEDS while the table is DELETING.
Why: writes land in a table that is going away, so a client sees acknowledged data that cannot be read back — and the negative path is untested.
Fix: PutItem handler's table-status gate (state lookup before the write).
Traces: dynamodb.put-item-on-deleting-table-rejected

## Evidence
Replay details, one per cause:
- attribute-name-length-limit: AssertionError: expected PutItem with a 65536-byte attribute name to be rejected, it succeeded. Expected code ValidationException, message '1 validation error detected: Attribute name is too large, must be less than 65536 bytes'.
- create-table-exceeds-limit-rejected: AssertionError: expected LimitExceededException, got 'ValidationException'.
- create-table-lsi-without-range-key: AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'TableDescription': {... 'KeySchema': [{'AttributeName': 'pk', 'KeyType': 'HASH'}], 'TableStatus': 'CREATING', 'LocalSecondaryIndexes': [{'IndexName': 'lsi1', ...}]}, 'ResponseMetadata': {'HTTPStatusCode': 200 ...}}.
- create-table-provisioned-without-throughput: expected 'One or more parameter values were invalid: ReadCapacityUnits and WriteCapacityUnits must both be specified when BillingMode is PROVISIONED', got 'One or more parameter values were invalid: Both ReadCapacityUnits and WriteCapacityUnits must be specified when BillingMode is PROVISIONED'.
- create-table-unused-attribute-definitions: expected 'One or more parameter values were invalid: Number of attributes in KeySchema does not exactly match number of attributes defined in AttributeDefinitions', got 'Some AttributeDefinitions are not used in any KeySchema: never_used'.
- gsi-key-attribute-must-be-scalar: expected "... failed to satisfy constraint: Member must satisfy enum value set: [B, N, S]", got "... [S, N, B]".
- gsi-multi-attribute-hash-key-query-without-every-attribute-rejected: five HASH attributes: message 'Invalid KeySchema for index five-hash: The fifth KeySchemaElement is not a RANGE key type'.
- provisioned-table-rejects-a-switch-with-no-throughput: expected 'One or more parameter values were invalid: ProvisionedThroughput must be specified when BillingMode is PROVISIONED', got the Both-ReadCapacityUnits/WriteCapacityUnits text.
- update-table-rejects-a-duplicate-index-name: expected 'Attempting to create an index which already exists', got 'Invalid GlobalSecondaryIndexUpdates: index dupname already exists on table <table>'.
- put-item-on-deleting-table-rejected: AssertionError: expected the call to be refused, but it succeeded.

Smithy grounding: CreateTable errors = InternalServerError, InvalidEndpointException, LimitExceededException, ResourceInUseException (+ service-common ValidationException); UpdateTable errors add ResourceNotFoundException; PutItem.Item is required and keyed by AttributeName.

## Steps to Reproduce
Boot the emulator and replay the named behavioral checks:
1. dynamodb.attribute-name-length-limit
2. dynamodb.create-table-exceeds-limit-rejected
3. dynamodb.create-table-lsi-without-range-key
4. dynamodb.create-table-provisioned-without-throughput
5. dynamodb.create-table-unused-attribute-definitions
6. dynamodb.gsi-key-attribute-must-be-scalar
7. dynamodb.gsi-multi-attribute-hash-key-query-without-every-attribute-rejected
8. dynamodb.provisioned-table-rejects-a-switch-with-no-throughput
9. dynamodb.update-table-rejects-a-duplicate-index-name
10. dynamodb.put-item-on-deleting-table-rejected
Each asserts the status/code/message of one refusal (or that a refusal happens at all).
