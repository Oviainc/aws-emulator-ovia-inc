Issue-Severity: HIGH
Issue-Commit: a2c0c19d1874b6d8f510c899cc1a51333f7d4b07

# DynamoDB key-schema and index-key refusals use invented messages, name the wrong member path, and report the wrong defect first (5 causes)

> _Note: this issue was identified against the code base at commit `a2c0c19d1874b6d8f510c899cc1a51333f7d4b07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateItem

## Description
Five net-new root causes in key validation, member-path rendering and defect precedence across PutItem, UpdateItem, BatchWriteItem and TransactWriteItems. Open issue 20261030T060227Z-dynamodb-validationexception-messages-diverge-from-the-wording-aws.md and 20261103T022510Z-dynamodb-control-plane-batch-transact-apis-legacy-parameters.md are adjacent; the suite did not mark any check below as owned by them.

=== Cause 1 of 5: a wrong-typed or missing key element is reported with a diagnostic AWS does not serve ===
AWS: when a supplied key does not match the table's KeySchema — wrong AttributeValue type for a key attribute, or a key attribute missing — the refusal is ValidationException with the fixed sentence "The provided key element does not match the schema". No attribute name, no expected/actual type, and the same sentence on the single-item path (PutItem/UpdateItem/DeleteItem) and inside BatchWriteItem and TransactWriteItems.
Emulator: it answers 'One or more parameter values were invalid: Type mismatch for key <name> expected: <T> actual: <T>' or 'One of the required keys was not given a value' instead, and inside TransactWriteItems it raises ValidationException where AWS cancels (or vice versa, see Cause 3).
Why: a client matching AWS's sentence never recognises the defect, and the leaked expected/actual detail is emulator-authored.
Fix: the key-validation component shared by the item, batch and transaction write handlers must emit the single schema-mismatch sentence.
Traces: update-item-key-schema-mismatch, batch-write-item-key-defects-message-diverges-from-put-item, transact-wrong-typed-key-cancels-and-empty-put-key-validates, ddb-wf-batch-write-item-missing-the-sort-key-rejected-whole.

=== Cause 2 of 5: the empty-string secondary-index-key refusal uses the wrong sentence for the update path ===
AWS: an update expression that sets a secondary index key attribute to the empty string is refused with "One or more parameter values are not valid. The update expression attempted to update a secondary index key to a value that is not supported. The AttributeValue for a key attribute cannot contain an empty string value." — the sentence names the UPDATE EXPRESSION and does NOT name the index or the index key.
Emulator: it serves "One or more parameter values are not valid. A value specified for a secondary index key is not supported. The AttributeValue for a key attribute cannot contain an empty string value. IndexName: <index>, IndexKey: <attr>" for the update path too, naming the index and key AWS withholds.
Why: the message differs on exact match, and the index/key names it discloses are not part of AWS's contract.
Fix: the empty-index-key validator must select the update-expression sentence when the defect arrives through UpdateItem / TransactWriteItems Update, and drop the IndexName/IndexKey suffix there.
Traces: put-batch-write-update-index-key-empty-value-rejected, transact-write-items-empty-index-key-validates-not-cancels, update-item-and-transact-update-empty-gsi-and-lsi-keys-rejected.

=== Cause 3 of 5: TransactWriteItems classifies key-attribute updates as validation instead of cancellation ===
AWS: an Update action inside TransactWriteItems that attempts to modify a key attribute is a per-item failure and comes back as TransactionCanceledException with that slot's cancellation reason — not a top-level ValidationException.
Emulator: it answers ValidationException for the whole call.
Why: a client reading CancellationReasons to find the offending slot gets nothing, and the HTTP error code differs from the one AWS serves for this class.
Fix: the TransactWriteItems handler's classification of per-item defects versus top-level request defects.
Traces: transact-write-items-update-key-and-empty-index-key-rejections.

=== Cause 4 of 5: constraint-violation messages name the serialized lowercase member and omit the container path ===
AWS: a Smithy constraint violation is rendered "<n> validation error(s) detected: Value '<v>' at '<memberPath>' failed to satisfy constraint: <clause>" where <memberPath> is the FULL path in the request as AWS renders it — for TransactWriteItems that is 'transactItems.1.member.put.tableName', naming the list index and action. For the per-table key of BatchWriteItem/BatchGetItem's RequestItems map, AWS does not report a tableName length constraint at all.
Emulator: it names only the leaf member ('tableName') with no container path, and it reports a tableName min-length constraint for a RequestItems map key that AWS refuses differently.
Why: a caller cannot locate which element of a 100-item transaction was rejected.
Fix: the constraint-violation renderer must carry the full member path (list index + member + action + leaf) from the deserializer.
Traces: batch-request-items-key-not-a-table-name-rejected, transact-write-items-invalid-table-name-names-member-path.

=== Cause 5 of 5: defect precedence and aggregation differ from AWS ===
AWS: when a request carries several defects, AWS reports them in a fixed order and with fixed aggregation — a ClientRequestToken length violation masks the other violations in the same TransactWriteItems request (so the message names one validation error, not two); request-shape validation is performed BEFORE the table is looked up, so a malformed request against a non-existent table is ValidationException, not ResourceNotFoundException; and the first fatal defect reported depends on the defect class, not on member order.
Emulator: it aggregates all violations ('2 validation errors detected: Value \'BOGUS\' at \'returnConsumedCapacity\' ...; Value at \'ClientRequestToken\' ...'), performs the table lookup first (ResourceNotFoundException 'Requested resource not found' where AWS serves ValidationException), and picks a different first defect.
Why: a client's error handling branches on the first reported defect; this makes an un-fixable request look like a missing-table problem and a masked violation look like two.
Fix: the request-validation pipeline ordering in the item/transaction handlers — shape validation, then masking rules, then resource lookup.
Traces: multiple-simultaneous-constraint-violations-rejected, transact-write-items-overlength-token-masks-other-violations-rejected, transact-write-items-first-fatal-defect-reported-depends-on-defect-class, request-validation-precedes-missing-table-lookup-rejected.

## Evidence
Cause 1 — update-item-key-schema-mismatch: expected 'The provided key element does not match the schema', got 'One or more parameter values were invalid: Type mismatch for key sk expected: S actual: N'. batch-write-item-key-defects-message-diverges-from-put-item: same expected sentence, got the Type-mismatch sentence. ddb-wf-batch-write-item-missing-the-sort-key-rejected-whole: raised ('ValidationException', 'One of the required keys was not given a value'), expected ('ValidationException', 'The provided key element does not match the schema').

Cause 2 — update-item-and-transact-update-empty-gsi-and-lsi-keys-rejected and transact-write-items-empty-index-key-validates-not-cancels: expected 'One or more parameter values are not valid. The update expression attempted to update a secondary index key to a value that is not supported. The AttributeValue for a key attribute cannot contain an empty string value.', got 'One or more parameter values are not valid. A value specified for a secondary index key is not supported. The AttributeValue for a key attribute cannot contain an empty string value. IndexName: gsi1, IndexKey: gpk'.

Cause 3 — transact-write-items-update-key-and-empty-index-key-rejections: "updating a key attribute: expected TransactionCanceledException, got 'ValidationException'".

Cause 4 — transact-write-items-invalid-table-name-names-member-path: expected "1 validation error detected: Value 'bad name' at 'transactItems.1.member.put.tableName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+", got the same clause at 'tableName'. batch-request-items-key-not-a-table-name-rejected: unexpected message "1 validation error detected: Value 'ab' at 'tableName' failed to satisfy constraint: Member must have length greater than or equal to 3".

Cause 5 — transact-write-items-overlength-token-masks-other-violations-rejected: expected "1 validation error detected: Value at 'ClientRequestToken' failed to satisfy constraint: Member must have length less than or equal to 36", got "2 validation errors detected: Value 'BOGUS' at 'returnConsumedCapacity' ...; Value at 'ClientRequestToken' ...". request-validation-precedes-missing-table-lookup-rejected: PutItem with Expected EQ and no value against a missing table gave ('ResourceNotFoundException', 'Requested resource not found'), expected ('ValidationException', '1 validation error detected: One or more parameter values were invalid: Value or AttributeValueList must be used with ComparisonOperator: EQ for Attribute: a'). transact-write-items-first-fatal-defect-reported-depends-on-defect-class: expected ValidationException, got 'ResourceNotFoundException'.

## Steps to Reproduce
1. Create a table with HASH pk (S) / RANGE sk (S) and a GSI gsi1 whose HASH is gpk (S).
2. ddb.update_item(TableName=t, Key={'pk':{'S':'a'},'sk':{'N':'1'}}, ...) — expect 'The provided key element does not match the schema'.
3. ddb.update_item(..., UpdateExpression='SET gpk = :e', ExpressionAttributeValues={':e':{'S':''}}) — expect the update-expression sentence with no IndexName/IndexKey suffix.
4. ddb.transact_write_items(TransactItems=[ok, {'Put': {'TableName': 'bad name', 'Item': {...}}}]) — read the member path in Error.Message.
5. ddb.transact_write_items(..., ClientRequestToken='x'*40, ReturnConsumedCapacity='BOGUS') — expect one validation error naming ClientRequestToken.
6. ddb.put_item(TableName='<never-created>', Item={...}, Expected={'a': {'ComparisonOperator': 'EQ'}}) — expect ValidationException, observe ResourceNotFoundException.
Then replay the dynamodb checks named in trace_ids.
