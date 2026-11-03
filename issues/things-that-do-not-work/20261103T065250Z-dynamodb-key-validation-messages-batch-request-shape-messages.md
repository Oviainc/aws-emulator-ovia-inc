Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.batch-write-item-key-defects-message-diverges-from-put-item dynamodb.ddb-wf-batch-write-item-missing-the-sort-key-rejected-whole dynamodb.update-item-key-schema-mismatch dynamodb.transact-wrong-typed-key-cancels-and-empty-put-key-validates dynamodb.malformed-n-in-key-is-top-level-validation-not-cancellation dynamodb.transact-oversized-hash-key-cancels-with-validation-error dynamodb.batch-request-shape-defects-rejected dynamodb.batch-write-over-25-rejected dynamodb.validation-messages-are-customer-intelligible dynamodb.ddb-sfa-remove-on-an-absent-key-is-a-no-op-unlike-set dynamodb.query-reverse-pages-at-1mb-in-descending-key-order dynamodb.exclusive-start-key-outside-range-and-missing-attribute-rejected dynamodb.restore-invalid-time
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.batch-write-item-key-defects-message-diverges-from-put-item|run#8|AssertionError|assert batch_put_message == batch_generic_message, (
Issue-Key: dynamodb.ddb-wf-batch-write-item-missing-the-sort-key-rejected-whole|run#10|AssertionError|assert got == ("ValidationException", "The provided key element does not match the schema"), (
Issue-Key: dynamodb.update-item-key-schema-mismatch|run#21|AssertionError|assert err3.get('Message') == KEY_MISMATCH, f'expected message {KEY_MISMATCH!r}, got {err3.get("Message")!r}'
Issue-Key: dynamodb.transact-wrong-typed-key-cancels-and-empty-put-key-validates|_assert_cancelled#10|AssertionError|assert (code, msg) == ("TransactionCanceledException", _CANCELLED), f"{label} gave {code!r}: {msg!r}"
Issue-Key: dynamodb.malformed-n-in-key-is-top-level-validation-not-cancellation|run#10|AssertionError|assert message == expected, f"expected AWS's exact GetItem message, got {message!r}"
Issue-Key: dynamodb.transact-oversized-hash-key-cancels-with-validation-error|run#13|AssertionError|assert (code, msg) == ("TransactionCanceledException", _CANCEL_ONE_MSG), (
Issue-Key: dynamodb.batch-request-shape-defects-rejected|run#6|AssertionError|assert (code, msg) == ("ValidationException", want), (
Issue-Key: dynamodb.batch-write-over-25-rejected|run#15|AssertionError|assert message == expected, f'expected message {expected!r}, got {message!r}'
Issue-Key: dynamodb.validation-messages-are-customer-intelligible|_error#7|AssertionError|raise AssertionError(f'real AWS rejects this call, but it SUCCEEDED and returned {result!r}')
Issue-Key: dynamodb.ddb-sfa-remove-on-an-absent-key-is-a-no-op-unlike-set|run#9|AssertionError|assert "Attributes" not in resp, (
Issue-Key: dynamodb.query-reverse-pages-at-1mb-in-descending-key-order|_batch_put#8|AssertionError|assert time.monotonic() < deadline, (
Issue-Key: dynamodb.exclusive-start-key-outside-range-and-missing-attribute-rejected|_error#11|AssertionError|raise AssertionError(f'expected the call to be refused, but it succeeded: {result!r}')
Issue-Key: dynamodb.restore-invalid-time|run#18|AssertionError|assert re.fullmatch(
Issue-Commit: ce4f28f15b4841beefbad6c596b9d2158570753e

# DynamoDB key-validation messages, batch request-shape messages, and 4 item-op behaviour defects (REMOVE creates an item, spurious UnprocessedItems)

> _Note: this issue was identified against the code base at commit `ce4f28f15b4841beefbad6c596b9d2158570753e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
Six net-new root causes left over from the DynamoDB sweep. All six must be closed before submitting.

Cause 1 of 6: key-schema / key-type refusals carry the wrong message or the wrong error class, and they differ between the single-item, batch and transact paths. AWS: a wrong-typed or structurally wrong key is 'The provided key element does not match the schema' on PutItem/UpdateItem/DeleteItem, and a batch whose member omits the sort key is refused whole with the SAME message; an over-long hash key is 'One or more parameter values were invalid: Size of hashkey has exceeded the maximum size limit of 2048 bytes'; a malformed N in a Key is a TOP-LEVEL ValidationException, not a cancellation; the wrong-typed key inside a transaction CANCELS (TransactionCanceledException) while an empty put key VALIDATES. Emulator: answers 'One or more parameter values were invalid: Type mismatch for key pk expected: S actual: N' and 'One of the required keys was not given a value' instead, emits 'Size of hashkey has exceeded the maximum size limit of2048 bytes' (missing the space before 2048), answers '1 validation error detected: The parameter cannot be converted to a numeric value: abc' for the malformed-N key, validates where AWS cancels, and answers 'The provided key element does not match the schema' where AWS reports a type mismatch. The fixed text is AWS's sentence; the variable parts are the key attribute name, the expected/actual types and the limit, taken from the request and the table schema. Why: callers key their retry/repair logic on these exact strings, and the batch-vs-single divergence makes one defect look like two. Fix: the key validation shared by the item, batch and transact write handlers. Checks: dynamodb.batch-write-item-key-defects-message-diverges-from-put-item, dynamodb.ddb-wf-batch-write-item-missing-the-sort-key-rejected-whole, dynamodb.update-item-key-schema-mismatch, dynamodb.transact-wrong-typed-key-cancels-and-empty-put-key-validates, dynamodb.malformed-n-in-key-is-top-level-validation-not-cancellation, dynamodb.transact-oversized-hash-key-cancels-with-validation-error.

Cause 2 of 6: batch request-shape refusals diverge, and one shape AWS refuses is accepted. AWS (Smithy: BatchWriteItem.RequestItems is a map with @length min 1, max 25, required): an empty RequestItems map is "1 validation error detected: Value at 'requestItems' failed to satisfy constraint: Member must have length greater than or equal to 1" and a 26-member batch is the corresponding 'less than or equal to 25' violation whose Value renders the submitted request map; a RequestItems key that is not a legal table name is reported for THAT constraint, not a length one. Emulator: answers "1 validation error detected: Value at 'RequestItems' failed to satisfy constraint: Member must have length greater than or equal to 1" (wrong member case/path), a differently rendered over-25 message, and for one shape it returns HTTP 200 with {'UnprocessedItems': {}} where AWS rejects the call outright. Fix: BatchWriteItem/BatchGetItem request-shape validation and its message rendering. Checks: dynamodb.batch-request-shape-defects-rejected, dynamodb.batch-write-over-25-rejected, dynamodb.validation-messages-are-customer-intelligible.

Cause 3 of 6: UpdateItem with only a REMOVE clause CREATES the item when the key is absent. AWS: REMOVE of an absent attribute on an absent key is a no-op -- no item is created and ALL_NEW returns nothing (unlike SET, which creates). Emulator: returned ALL_NEW {'id': {'S': 'user-004'}, 'timestamp': {'N': '1700000006'}} -- it materialised a key-only item. Why: phantom rows appear in scans and counts, and idempotent cleanup code starts creating data. Fix: UpdateItem's update-expression application in the item write path. Check: dynamodb.ddb-sfa-remove-on-an-absent-key-is-a-no-op-unlike-set.

Cause 4 of 6: BatchWriteItem reports UnprocessedItems for a batch that is within every limit. AWS: a 25-item batch of small items under the table's capacity is written in full and UnprocessedItems comes back empty. Emulator: 'BatchWriteItem left 6 items unprocessed', so the data the next operation depends on is simply missing. Why: callers that trust an empty UnprocessedItems lose writes; callers that retry see non-deterministic partial success. Fix: BatchWriteItem's write loop / unprocessed accounting. Check: dynamodb.query-reverse-pages-at-1mb-in-descending-key-order.

Cause 5 of 6: an ExclusiveStartKey that is not a legal key for the table is accepted. AWS: a start key outside the key schema, or missing a key attribute, is refused with ValidationException. Emulator: returned HTTP 200 with {'Items': [], 'Count': 0, 'ScannedCount': 0}. Why: a paging bug reads as 'no more data' instead of an error. Fix: ExclusiveStartKey validation in Query/Scan. Check: dynamodb.exclusive-start-key-outside-range-and-missing-attribute-rejected.

Cause 6 of 6: RestoreTableToPointInTime's invalid-restore-time refusal diverges. AWS: an out-of-window RestoreDateTime is refused with the message 'An invalid restore time was specified. RestoreDateTime must be between EarliestRestorableDateTime and LatestRestorableDateTime' (recorded behavior; the model lists only InvalidRestoreTimeException/ValidationException for the operation). Emulator: does not answer with that refusal. Fix: RestoreTableToPointInTime's restore-window validation. Check: dynamodb.restore-invalid-time.

## Evidence
Replayed behavioral suite, service heading '### dynamodb'. Verbatim:
- 'BatchWriteItem PutRequest wrong-typed partition key: expected message \'The provided key element does not match the schema\', got \'One or more parameter values were invalid: Type mismatch for key pk expected: S actual: N\''
- 'a batch whose second put lacks the sort key raised (\'ValidationException\', \'One of the required keys was not given a value\'), expected (\'ValidationException\', \'The provided key element does not match the schema\')'
- 'expected message \'The provided key element does not match the schema\', got \'One or more parameter values were invalid: Type mismatch for key sk expected: S actual: N\'' (UpdateItem)
- 'TransactWriteItems Delete with a 2049-byte hash key gave \'ValidationException\': \'One or more parameter values were invalid: Size of hashkey has exceeded the maximum size limit of2048 bytes\''
- 'Delete with an N key on an S-keyed table gave \'ValidationException\': \'The provided key element does not match the schema\'' (expected a cancellation)
- 'expected AWS\'s exact GetItem message, got \'1 validation error detected: The parameter cannot be converted to a numeric value: abc\''
- 'BatchWriteItem with an empty RequestItems map gave \'ValidationException\': "1 validation error detected: Value at \'RequestItems\' failed to satisfy constraint: Member must have length greater than or equal to 1"'
- 'real AWS rejects this call, but it SUCCEEDED and returned {\'UnprocessedItems\': {}, ... \'HTTPStatusCode\': 200 ...}'
- 'REMOVE of an absent attribute on an absent key returned ALL_NEW {\'id\': {\'S\': \'user-004\'}, \'timestamp\': {\'N\': \'1700000006\'}}, expected no item to have been created'
- 'BatchWriteItem left 6 items unprocessed'
- 'expected the call to be refused, but it succeeded: {\'Items\': [], \'Count\': 0, \'ScannedCount\': 0, ... 200 ...}'
- 'An invalid restore time was specified. RestoreDateTime must be between EarliestRestorableDateTime and LatestRestorableDateTime'

## Steps to Reproduce
1. Boot the emulator; point botocore at its dynamodb endpoint.
2. Re-replay the suite and filter to the check ids in trace_ids.
3. Spot checks: (a) BatchWriteItem with a PutRequest whose pk is {'N':'1'} on an S-keyed table -> expect 'The provided key element does not match the schema'; (b) BatchWriteItem(RequestItems={}) -> message must name 'requestItems'; (c) UpdateItem(Key=absent-key, UpdateExpression='REMOVE #a', ReturnValues='ALL_NEW') -> expect no Attributes and no item created; (d) BatchWriteItem of 25 small puts on an on-demand table -> expect UnprocessedItems == {}; (e) Scan(ExclusiveStartKey={'nope': {'S':'x'}}) -> expect ValidationException.
