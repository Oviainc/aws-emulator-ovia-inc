Issue-Severity: HIGH
Issue-Commit: a2c0c19d1874b6d8f510c899cc1a51333f7d4b07

# DynamoDB data-plane limits are unenforced or wrongly worded: size/nesting, the N and set domains, Query/Scan request defects, PartiQL semantics, vector metering and batch shape (8 causes)

> _Note: this issue was identified against the code base at commit `a2c0c19d1874b6d8f510c899cc1a51333f7d4b07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
Eight net-new root causes where the emulator ACCEPTS input real AWS refuses, or refuses it with text AWS does not serve. Open issue 20261103T022510Z-dynamodb-data-plane-validation-gaps-partiql-semantics-number.md and 20261030T060227Z-dynamodb-validationexception-messages-diverge-from-the-wording-aws.md describe neighbouring ground; the suite did not mark any of the checks below as owned by them, so these replay keys are not covered.

=== Cause 1 of 8: item/request/key-size and nesting-depth refusals carry invented text, and the 4 MB transaction ceiling is not enforced at all ===
AWS: the 400 KB item ceiling, the 2048-byte partition-key ceiling, the 32-level document nesting ceiling and the 4 MB aggregate TransactWriteItems ceiling are each refused with ValidationException, with AWS's fixed sentence per class; on TransactWriteItems and BatchWriteItem these are TOP-LEVEL validation of the whole call, not a per-item cancellation, and the size defect is ordered ahead of (or behind, per class) the other defects exactly as AWS orders it.
Emulator: the refusals that fire carry hand-written text — 'Item size has exceeded the maximum allowed size', '1 validation error detected: Nesting Levels have exceeded supported limits: Attributes in the item have nested levels beyond supported limit', 'One or more parameter values were invalid: Size of hashkey has exceeded the maximum size limit of2048 bytes' (note the missing space before 2048) — and the 4 MB aggregate transaction is not refused at all (HTTP 200).
Why: a client cannot tell which limit it hit, and a transaction larger than AWS's hard ceiling commits here and then fails in production.
Fix: the size/nesting validators shared by the item, batch and transaction handlers, their message renderer, and the aggregate-payload check in TransactWriteItems.
Traces: item-size-limit-enforced-at-400kb, partiql-update-growth-past-400kb-matches-update-item, transact-write-items-update-grows-item-over-size-limit, update-item-set-list-append-add-exceeding-400kb-rejected, oversized-item-to-a-missing-table-not-found-on-put-rejected-on-batch-write, transact-write-aggregate-size-over-4mb-rejected, transact-oversized-hash-key-cancels-with-validation-error, nesting-depth-error-precedence-across-malformed-requests, nesting-depth-limit-covers-transact-condition-check-and-legacy-expected-values, transact-write-items-nesting-violation-is-top-level-validation-not-cancellation, batch-write-item-rejects-whole-call-for-one-oversized-nested-put.

=== Cause 2 of 8: the N (number) domain is not enforced on write, and the refusals that do fire are worded wrongly ===
AWS: a DynamoDB N value carries at most 38 significant digits with an exponent in [-128, 126]; a value outside that domain, a non-numeric string in an N slot, an out-of-range zero exponent and an arithmetic result that overflows are each ValidationException with AWS's fixed sentence per class, and a malformed N in a KEY is top-level validation, not a cancellation.
Emulator: number-canonicalization-boundaries, number-precision-limited-to-38-digits and zero-with-out-of-range-exponent-rejected all succeed with HTTP 200; the PartiQL parameter binder also accepts a non-numeric and an out-of-range N parameter; and where a refusal fires it is emulator text ('1 validation error detected: The parameter cannot be converted to a numeric value: abc', '1 validation error detected: Number overflow. Attempting to store a number with magnitude larger than supported range').
Why: items real AWS will never accept are written and read back, so data created against the emulator is not migratable, and a client branching on the refusal message cannot.
Fix: N canonicalisation/validation in the AttributeValue deserializer shared by the item handlers and the PartiQL parameter binder.
Traces: number-canonicalization-boundaries, number-precision-limited-to-38-digits, zero-with-out-of-range-exponent-rejected, update-item-malformed-or-overflowing-number-rejected, malformed-n-in-key-is-top-level-validation-not-cancellation, ddb-frc-non-numeric-number-operand-rejected, execute-statement-rejects-non-numeric-n-parameter, execute-statement-rejects-out-of-range-n-parameter.

=== Cause 3 of 8: the set-member domain is not enforced ===
AWS: SS/NS/BS values may not carry an empty member or a repeated member; such a value is refused with ValidationException wherever it appears, including as a ConditionExpression value.
Emulator: the write and the condition value are both accepted.
Why: a set that AWS cannot represent is stored, and set-cardinality logic diverges.
Fix: set-member validation in the AttributeValue deserializer.
Traces: empty-and-duplicate-set-members-rejected-for-ss-ns-and-bs.

=== Cause 4 of 8: assorted Query/Scan/DescribeTable/RestoreTable request defects are not validated, or name the wrong member ===
AWS: IN with more than 100 operands is refused; an ExclusiveStartKey outside the queried range or missing a key attribute is refused; a bare reserved word used as a sort key in a key condition is refused; Select=SPECIFIC_ATTRIBUTES with no ProjectionExpression and Select=ALL_PROJECTED_ATTRIBUTES on a base-table scan each have AWS's own fixed sentence; Smithy models Scan.TotalSegments with @range min 1 max 1000000 and AWS renders the violation as "1 validation error detected: Value at 'TotalSegments' failed to satisfy constraint: Member must have value less than or equal to 1000000" — capitalised member, value NOT interpolated; DescribeTable given another region's table ARN is ValidationException "1 validation error detected: Invalid AWS region in '<arn>'"; RestoreTableToPointInTime outside the restorable window has AWS's sentence; and a BatchWriteItem of a modest batch is absorbed rather than left unprocessed.
Emulator: the first four succeed; the Select refusals carry invented text; TotalSegments names the lower-cased 'totalSegments' and interpolates the value; the cross-region DescribeTable answers ResourceNotFoundException; and BatchWriteItem leaves 6 items unprocessed for a batch AWS absorbs, which prevents the reverse-paging check from even setting up.
Why: invalid requests are accepted so they fail only in production, and the refusals that do occur name members and values AWS does not.
Fix: Query/Scan input validation, the Select/ProjectionExpression consistency check, the shared constraint-violation renderer (member-path casing, no value interpolation where AWS omits it), DescribeTable ARN parsing, and BatchWriteItem's unprocessed-items policy.
Traces: ddb-ss-in-operator-with-101-operands-rejected, exclusive-start-key-outside-range-and-missing-attribute-rejected, ddb-sfa-bare-reserved-sort-key-timestamp-in-key-condition-rejected, ddb-frc-specific-attributes-without-a-projection-rejected, ddb-ss-all-projected-attributes-on-a-base-table-scan-rejected, ddb-gsi-scan-total-segments-above-one-million-rejected, ddb-ltr-describe-table-with-another-regions-arn-rejected, restore-invalid-time, query-reverse-pages-at-1mb-in-descending-key-order.

=== Cause 5 of 8: PartiQL evaluation semantics differ ===
AWS: in PartiQL over DynamoDB a MISSING attribute is not NULL, so "WHERE a IS NULL" matches only items whose attribute is the NULL AttributeValue type; a PartiQL UPDATE ... REMOVE against a key that does not exist is a no-op and creates nothing; and a SELECT that projects a nested path renders the result keyed by the path AS WRITTEN, including the list index. Recorded behavior for the exact rendering.
Emulator: IS NULL also matches items whose attribute is absent; REMOVE of an absent attribute on an absent key returns ALL_NEW and CREATES the item; and a nested list path is rendered as the bare attribute name.
Why: reads return extra rows, a delete-shaped statement creates phantom items, and a caller cannot correlate projected values back to the paths it asked for.
Fix: the PartiQL evaluator behind ExecuteStatement — predicate evaluation, REMOVE application, projection rendering.
Traces: ddb-sfa-partiql-is-not-null-counts-a-missing-attribute-as-not-null, ddb-sfa-remove-on-an-absent-key-is-a-no-op-unlike-set, partiql-select-projects-nested-map-and-list-paths.

=== Cause 6 of 8: PartiQL index-projection and operand-type enforcement is absent ===
AWS: a PartiQL SELECT through an index qualifier may only filter on attributes that index projects — filtering on a non-projected attribute of an INCLUDE GSI or a KEYS_ONLY LSI is ValidationException; relational operators over complex-type (M/L/set) operands are refused; and size()/attribute_type over an incompatible operand is refused.
Emulator: all of these succeed — the non-projected filter returns the item at HTTP 200, and the complex-operand and incompatible-size comparisons return an empty page at HTTP 200.
Why: a client ships statements AWS refuses and relies on reading attributes the index does not hold.
Fix: index-projection and operand-type checks in the PartiQL planner/validator.
Traces: partiql-relational-operators-reject-complex-type-operands, partiql-select-gsi-include-projection-nonprojected-filter-rejected, partiql-select-lsi-keys-only-nonprojected-filter-and-in-between-key-forms, partiql-select-through-lsi-qualifier, partiql-size-and-attribute-type-select-exact-sets-and-reject-incompatible-size.

=== Cause 7 of 8: vector-index writes are neither validated nor metered ===
AWS: a PutItem whose vector attribute does not satisfy the index's configured dimension/schema is ValidationException, and a vector write is metered per index over that attribute with a per-write byte floor reported in the consumed-capacity breakdown (VectorIndexes / VectorWriteRequestBytes).
Emulator: the malformed vector write succeeds at HTTP 200 and the consumed-capacity response carries no vector metering at all — VectorIndexes is {} and VectorWriteRequestBytes is absent.
Why: invalid embeddings are stored and vector capacity planning is impossible.
Fix: PutItem vector validation and the consumed-capacity accounting in the item write path/state.
Traces: put-item-vector-write-validation-and-search-projection, vector-write-request-bytes-floor-and-per-dimension-cost.

=== Cause 8 of 8: BatchWriteItem request-shape refusals are worded wrongly ===
AWS: an empty RequestItems map and a batch of more than 25 WriteRequests are each refused with ValidationException and AWS's own rendering of the constraint — for the over-25 case AWS interpolates the whole rejected RequestItems value into the "Value '<...>' at 'requestItems' failed to satisfy constraint: Member must have length less than or equal to 25" sentence; for the empty map AWS's sentence differs from the emulator's.
Emulator: the empty map answers "1 validation error detected: Value at 'RequestItems' failed to satisfy constraint: Member must have length greater than or equal to 1" and the over-25 case does not reproduce AWS's rendering of the rejected value.
Why: exact-match error handling and SDK error parsing differ.
Fix: the BatchWriteItem constraint-violation renderer.
Traces: batch-request-shape-defects-rejected, batch-write-over-25-rejected.

## Evidence
Cause 1 — transact-write-aggregate-size-over-4mb-rejected: expected the call to be refused, but it succeeded (HTTP 200). item-size-limit-enforced-at-400kb: got 'Item size has exceeded the maximum allowed size'. transact-oversized-hash-key-cancels-with-validation-error: got 'One or more parameter values were invalid: Size of hashkey has exceeded the maximum size limit of2048 bytes'. batch-write-item-rejects-whole-call-for-one-oversized-nested-put / transact-write-items-nesting-violation-is-top-level-validation-not-cancellation: got '1 validation error detected: Nesting Levels have exceeded supported limits: Attributes in the item have nested levels beyond supported limit'. oversized-item-to-a-missing-table-not-found-on-put-rejected-on-batch-write: PutItem of an oversized item to the missing table gave 'ValidationException': 'Item size has exceeded the maximum allowed size'.

Cause 2 — number-precision-limited-to-38-digits: real AWS rejects this call, but it SUCCEEDED (HTTP 200). number-canonicalization-boundaries / zero-with-out-of-range-exponent-rejected: SUCCEEDED (HTTP 200). execute-statement-rejects-non-numeric-n-parameter / execute-statement-rejects-out-of-range-n-parameter: SUCCEEDED with {'Items': []} at HTTP 200. ddb-frc-non-numeric-number-operand-rejected: the call succeeded. malformed-n-in-key-is-top-level-validation-not-cancellation: got '1 validation error detected: The parameter cannot be converted to a numeric value: abc'. update-item-malformed-or-overflowing-number-rejected: got '1 validation error detected: Number overflow. Attempting to store a number with magnitude larger than supported range'.

Cause 3 — empty-and-duplicate-set-members-rejected-for-ss-ns-and-bs: PutItem whose ConditionExpression value is a string set with a repeated member: the call was ACCEPTED; real AWS rejects it.

Cause 4 — ddb-ss-in-operator-with-101-operands-rejected: the call SUCCEEDED. exclusive-start-key-outside-range-and-missing-attribute-rejected: succeeded with {'Items': [], 'Count': 0} at HTTP 200. ddb-sfa-bare-reserved-sort-key-timestamp-in-key-condition-rejected: the call succeeded. ddb-gsi-scan-total-segments-above-one-million-rejected: answered "1 validation error detected: Value '1000001' at 'totalSegments' failed to satisfy constraint: Member must have value less than or equal to 1000000", expected "1 validation error detected: Value at 'TotalSegments' failed to satisfy constraint: Member must have value less than or equal to 1000000". ddb-ltr-describe-table-with-another-regions-arn-rejected: answered ('ResourceNotFoundException', 'Requested resource not found: Table: <name> not found'), expected ('ValidationException', "1 validation error detected: Invalid AWS region in '<arn>'"). ddb-frc-specific-attributes-without-a-projection-rejected: got 'One or more parameter values were invalid: Select type SPECIFIC_ATTRIBUTES requires the attributes to be listed in ProjectionExpression'. ddb-ss-all-projected-attributes-on-a-base-table-scan-rejected: got 'One or more parameter values were invalid: Select type ALL_PROJECTED_ATTRIBUTES is allowed only when an index is scanned'. query-reverse-pages-at-1mb-in-descending-key-order: BatchWriteItem left 6 items unprocessed. restore-invalid-time: 'An invalid restore time was specified. RestoreDateTime must be between EarliestRestorableDateTime and LatestRestorableDateTime'.

Cause 5 — ddb-sfa-partiql-is-not-null-counts-a-missing-attribute-as-not-null: IS NULL returned ('user-004','website-001','website-002','website-003'), expected only the NULL-typed ('user-004',). ddb-sfa-remove-on-an-absent-key-is-a-no-op-unlike-set: REMOVE returned ALL_NEW {'id': {'S':'user-004'}, 'timestamp': {'N':'1700000006'}}, expected no item to have been created. partiql-select-projects-nested-map-and-list-paths: returned [{'b':..., 'l': {'N':'200'}, 'x':...}], expected the key 'l[1]'.

Cause 6 — partiql-select-gsi-include-projection-nonprojected-filter-rejected: expected the statement to be refused, but it succeeded and returned the item at HTTP 200. partiql-relational-operators-reject-complex-type-operands / partiql-size-and-attribute-type-select-exact-sets-and-reject-incompatible-size: SUCCEEDED with {'Items': []} at HTTP 200. partiql-select-through-lsi-qualifier: the call was ACCEPTED; real AWS rejects it.

Cause 7 — put-item-vector-write-validation-and-search-projection: expected the call to be refused, but it succeeded at HTTP 200. vector-write-request-bytes-floor-and-per-dimension-cost: VectorWriteRequestBytes was None, expected the 1024-byte floor.

Cause 8 — batch-request-shape-defects-rejected: BatchWriteItem with an empty RequestItems map gave "1 validation error detected: Value at 'RequestItems' failed to satisfy constraint: Member must have length greater than or equal to 1". batch-write-over-25-rejected: expected AWS's rendering interpolating the whole rejected RequestItems value into the length-25 constraint sentence.

## Steps to Reproduce
1. ddb.transact_write_items with an aggregate payload over 4 MB — expect ValidationException, observe HTTP 200.
2. ddb.put_item(Item={'pk': {'S':'x'}, 'n': {'N': '1' + '0'*40}}) — expect ValidationException, observe HTTP 200.
3. ddb.put_item with an SS value holding a repeated member — expect ValidationException, observe acceptance.
4. ddb.scan(TableName=t, Segment=0, TotalSegments=1000001) and read Error.Message — the member must be 'TotalSegments' with no value interpolated.
5. ddb.describe_table(TableName='arn:aws:dynamodb:us-east-1:<acct>:table/<t>') against the us-west-2 endpoint and read Error.Code — expect ValidationException.
6. ddb.execute_statement(Statement='SELECT * FROM "<t>" WHERE a IS NULL') over a table holding both NULL-typed and absent values.
7. ddb.execute_statement(Statement='UPDATE "<t>" REMOVE x WHERE pk=\'absent\' ... RETURNING ALL NEW *') and check no item was created.
8. ddb.put_item of an item whose vector attribute has the wrong dimension, then PutItem with ReturnConsumedCapacity=INDEXES and read VectorIndexes / VectorWriteRequestBytes.
9. ddb.batch_write_item(RequestItems={}) and with 26 WriteRequests, and read Error.Message.
Then replay the dynamodb checks named in trace_ids.
