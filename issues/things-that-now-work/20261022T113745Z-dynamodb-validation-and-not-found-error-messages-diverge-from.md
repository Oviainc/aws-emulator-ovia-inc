Issue-Source: trace-sweep-divergence
Issue-Trace: ddbdepth-query-filter-limit
Issue-Step: task#12
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ddbdepth-query-filter-limit dynamodb-batch-caps-and-unprocessed dynamodb-validation-error-wording-across-the-data-plane ddbctl-resource-policy-deny-vs-not-found dynamodb-item-crud-and-expressions dynamodb-item-crud-roundtrip-with-conditions dynamodb-validation-message-names-the-violated-concept ddbval-item-over-400kb-rejected-not-stored ddbval-nesting-depth-over-32-levels-rejected dynamodb-attribute-value-union-arity-rejected dynamodb-return-values-all-new-rejected-on-put-and-delete dynamodb-validation-before-table-lookup-putitem-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|GetItem|4xx->4xx|$.message
Issue-Key: dynamodb|BatchGetItem|4xx->4xx|$.message
Issue-Key: dynamodb|TransactGetItems|4xx->4xx|$.message
Issue-Key: dynamodb|TransactWriteItems|4xx->4xx|$.message
Issue-Key: dynamodb|DeleteItem|4xx->4xx|$.message
Issue-Related: 20261022T113745Z-dynamodb-tables-created-with-secondary-indexes-never-materialize-later.md
Issue-Commit: 38aaeb6c250810a0cf0ab5b6ac35c7222e2099c5

# Validation and not-found error messages diverge from the AWS wording rules

> _Note: this issue was identified against the code base at commit `38aaeb6c250810a0cf0ab5b6ac35c7222e2099c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Query

## Description
Status codes and error codes are correct in these clusters; the message does not follow AWS's documented construction rules. Two rules are at stake, both derived from the request rather than free text: (a) a constraint violation on a modeled member is reported as 'N validation error(s) detected: Value ... at '<wire member path>' failed to satisfy constraint: <constraint>', where the member path uses the WIRE (capitalized) member names from the Smithy model; (b) data-plane ResourceNotFoundException carries the fixed text 'Requested resource not found' with no resource detail appended. The emulator lower-cases member paths, echoes values AWS does not (and omits values AWS does), appends internal detail, and drops the '1 validation error detected:' prefix. This bundle has 7 causes; all 7 must be closed. In every cause the recorded text is an example of the rule - the rule is the requirement.

Cause 1 of 7: Query Limit range violation uses a lower-cased member path and echoes the value.
AWS: Smithy: Query input Limit targets PositiveIntegerObject with range {min: 1}; the member is 'Limit' on the wire. Recorded: "1 validation error detected: Value at 'Limit' failed to satisfy constraint: Member must have value greater than or equal to 1".
Emulator: "1 validation error detected: Value '0' at 'limit' failed to satisfy constraint: ..." - path 'limit', plus a value echo.
Why: clients and test suites match on the member path to attribute the error to a field; a lower-cased path does not correspond to any modeled member.
Fix: the shared validation-error formatter (emit the model's wire member name, and follow AWS's rule about when the value is echoed).
Traces: ddbdepth-query-filter-limit#task#12 (3 steps in cluster, incl. dynamodb-batch-caps-and-unprocessed#task#13).

Cause 2 of 7: data-plane ResourceNotFoundException appends the table name. AWS: Smithy: GetItem error ResourceNotFoundException (400); recorded message exactly 'Requested resource not found'. Emulator: 'Requested resource not found: Table: <name> not found'. Why: the message leaks resource detail AWS withholds, and clients that compare the message (or hash it) see an unknown error. Fix: the ResourceNotFoundException formatter on the data-plane path. Traces: ddbctl-resource-policy-deny-vs-not-found#task#11 (12 steps in cluster, incl. dynamodb-validation-error-wording-across-the-data-plane#task#9..#task#13).

Cause 3 of 7: the item-size message appends byte counts. AWS: Smithy: PutItem + ValidationException; recorded message exactly 'Item size has exceeded the maximum allowed size'. Emulator: 'Item size has exceeded the maximum allowed size of 409600 bytes: 460828 bytes'. Why: as Cause 2 - extra internal detail in a message AWS keeps fixed. Fix: the item-size validation's error formatter. Traces: ddbval-item-over-400kb-rejected-not-stored#task#4 (12 steps in cluster, incl. dynamodb-attribute-value-union-arity-rejected#task#4..#task#8, dynamodb-validation-before-table-lookup-putitem-rejected#task#1).

Cause 4 of 7: BatchGetItem's 100-key cap message lower-cases the member path and echoes an item count. AWS: Smithy: BatchGetItem input RequestItems; recorded message "1 validation error detected: Value at 'RequestItems.<table>.member.Keys' failed to satisfy constraint: Member must have length less than or equal to 100". Emulator: "Value '101 items' at 'requestItems.<table>.member.keys' ...". Why: as Cause 1. Fix: the validation-error formatter's member-path construction for nested map/list members. Traces: dynamodb-batch-caps-and-unprocessed#task#8.

Cause 5 of 7: TransactGetItems' cap message omits the value echo AWS includes. AWS: Smithy: TransactGetItems input TransactItems; the recorded message is the '1 validation error detected: Value '<the rejected collection>' at ... failed to satisfy constraint' form, i.e. AWS echoes the offending value. Emulator: a message without that echo. Why: as Cause 1 - the formatter does not implement the echo rule, so no client can parse which value was rejected. Fix: the validation-error formatter (value echo for collection members). Traces: dynamodb-batch-caps-and-unprocessed#task#11.

Cause 6 of 7: TransactWriteItems' cap message has the same defect. AWS/Emulator/Why/Fix as Cause 5, for TransactWriteItems input TransactItems. Traces: dynamodb-batch-caps-and-unprocessed#task#12.

Cause 7 of 7: the ReturnValues rejection drops the '1 validation error detected:' prefix. AWS: Smithy: DeleteItem input ReturnValues targets the ReturnValue enum (NONE|ALL_OLD|UPDATED_OLD|ALL_NEW|UPDATED_NEW) but DeleteItem accepts only NONE or ALL_OLD; recorded message '1 validation error detected: ReturnValues can only be ALL_OLD or NONE'. Emulator: 'ReturnValues can only be ALL_OLD or NONE for DeleteItem, got ALL_NEW' - no prefix, extra detail. Why: clients that detect the validation family by the prefix do not recognize the error. Fix: route this check through the shared validation-error formatter. Traces: dynamodb-return-values-all-new-rejected-on-put-and-delete#task#6 (4 steps in cluster, incl. dynamodb-validation-error-wording-across-the-data-plane#task#5).

## Evidence
All seven clusters match on status and error code and differ only in the message, so each is filed against a known construction rule.
Representative: fixture ddbdepth-query-filter-limit step task#12, Query, 400/400: $.message expected "1 validation error detected: Value at 'Limit' failed to satisfy constraint: Member must have value greater than or equal to 1", got "1 validation error detected: Value '0' at 'limit' failed to satisfy constraint: Member must have value greater than or equal to 1".
Cause 2: expected 'Requested resource not found', got 'Requested resource not found: Table: <name> not found' (12 steps).
Cause 3: expected 'Item size has exceeded the maximum allowed size', got '... of 409600 bytes: 460828 bytes' (12 steps).
Cause 4: expected path 'RequestItems.<table>.member.Keys' with no value echo, got 'requestItems.<table>.member.keys' with "Value '101 items'".
Causes 5/6: TransactGetItems / TransactWriteItems cap messages - expected the '1 validation error detected: Value '<collection>' at ...' form, actual message omits the echoed collection.
Cause 7: expected '1 validation error detected: ReturnValues can only be ALL_OLD or NONE', got 'ReturnValues can only be ALL_OLD or NONE for DeleteItem, got ALL_NEW'.
The recorded texts (byte counts, table names, the serialized collection dumps) are examples; the requirements are the wire member-path casing, the '<N> validation error(s) detected:' prefix, the value-echo rule and the fixed not-found/item-size texts.

## Steps to Reproduce
1. Replay the wire corpus.
2. Fixture ddbdepth-query-filter-limit, step task#12: Query with Limit=0 and compare the message's member path ('limit' vs 'Limit') and value echo.
3. Fixture ddbctl-resource-policy-deny-vs-not-found, step task#11: GetItem on a missing table - the message appends 'Table: <name> not found'.
4. Fixture ddbval-item-over-400kb-rejected-not-stored, step task#4: PutItem a >400KB item - the message appends byte counts.
5. Fixture dynamodb-batch-caps-and-unprocessed, steps task#8, task#11, task#12: exceed the 100-key / 100-item caps for BatchGetItem, TransactGetItems and TransactWriteItems.
6. Fixture dynamodb-return-values-all-new-rejected-on-put-and-delete, step task#6: DeleteItem with ReturnValues=ALL_NEW - the '1 validation error detected:' prefix is missing.
