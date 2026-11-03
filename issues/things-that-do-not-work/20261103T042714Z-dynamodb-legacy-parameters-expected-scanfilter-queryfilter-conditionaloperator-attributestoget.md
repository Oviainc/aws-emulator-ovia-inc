Issue-Severity: HIGH
Issue-Commit: a2c0c19d1874b6d8f510c899cc1a51333f7d4b07

# DynamoDB legacy parameters (Expected, ScanFilter/QueryFilter, ConditionalOperator, AttributesToGet) are mis-validated and their filter semantics differ from the expression functions (5 causes)

> _Note: this issue was identified against the code base at commit `a2c0c19d1874b6d8f510c899cc1a51333f7d4b07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Scan

## Description
Five net-new root causes in the legacy (pre-expression) parameter surface. Open issue 20261103T022510Z-dynamodb-control-plane-batch-transact-apis-legacy-parameters.md covers neighbouring ground but the suite did not mark any check below as owned by it.

=== Cause 1 of 5: legacy-parameter refusals are emulator-authored and the enum defect is ordered wrongly ===
AWS: legacy-parameter defects are ValidationException with fixed sentences — a ComparisonOperator given the wrong number of AttributeValueList entries is "One or more parameter values were invalid: Invalid number of argument(s) for the <OP> ComparisonOperator"; Expected with Exists=true and no Value is AWS's own fixed sentence; ConditionalOperator without the companion legacy map, and ConditionalOperator mixed with an expression parameter, each have their own sentence; and when a request carries BOTH an invalid ConditionalOperator enum value and another defect, AWS's ordering determines which is reported. Mixing legacy and expression parameters is "Can not use both expression and non-expression parameters in the same request: Non-expression parameters: {<every legacy parameter present, AWS's order>} Expression parameters: {<every expression parameter present>}" — the list must name EVERY legacy parameter in the request, including ConditionalOperator.
Emulator: it serves hand-written sentences instead ('One or more parameter values were invalid: NOT_NULL takes 0 value(s) in AttributeValueList, got 1'; 'One or more parameter values were invalid: Value must also be used when Exists is true for attribute attr'; 'ConditionalOperator can only be used together with ScanFilter'; 'ConditionalOperator can only be used together with Expected'), omits ConditionalOperator from the non-expression parameter list, and reports the Smithy enum violation where AWS reports the ConditionalOperator defect.
Why: a client branching on these messages, or on which defect comes first, sees text AWS never serves.
Fix: the legacy-parameter validator in the Scan/Query/PutItem/UpdateItem/DeleteItem handlers and the mixed-parameter message builder (it must enumerate all legacy parameters present).
Traces: ddb-sfa-legacy-not-null-with-a-value-list-rejected, legacy-expected-exists-value-comparator-matrix, write-conditional-operator-needs-two-expected-conditions-rejected, scan-query-conditional-operator-needs-two-filter-conditions-rejected, scan-query-legacy-and-expression-mixing-names-every-parameter-rejected, ddb-ss-scan-filter-mixed-with-filter-expression-rejected, conditionaloperator-enum-error-precedes-nesting-depth-in-legacy-expected, scan-filter-conditional-operator-document-value-equality.

=== Cause 2 of 5: legacy filter comparators treat a MISSING attribute as matching ===
AWS: a legacy ScanFilter/QueryFilter comparator evaluates only over items that HAVE the attribute — NOT_CONTAINS and NULL/NOT_NULL exclude every item missing the attribute, matching the expression functions attribute_exists / attribute_not_exists combined with the negated comparator; NE is the documented exception.
Emulator: NOT_CONTAINS returns items that do not carry the attribute at all, so the legacy result set is strictly larger than the expression result set over the same data.
Why: a client migrating from legacy filters to FilterExpression gets a different answer for the same intent, and sparse-attribute scans leak rows.
Fix: the legacy comparator evaluator in the Scan/Query filter path — gate each comparator on attribute presence.
Traces: ddb-sfa-legacy-scan-filter-not-null-and-null-match-the-expression-functions, scan-filter-ne-and-not-contains-on-missing-attribute.

=== Cause 3 of 5: binary attributes are not compared as unsigned bytes ===
AWS: B/BS values are compared byte-by-byte as UNSIGNED bytes, so CONTAINS over a binary value finds a subsequence containing 0x00.
Emulator: CONTAINS of a 0x00 byte returns no items, so the subsequence that holds it is never found.
Why: binary keys and filters silently return the wrong set.
Fix: the AttributeValue comparison helpers used by the filter evaluator (unsigned byte ordering / subsequence search for B).
Traces: scan-filter-binary-attribute-unsigned-byte-order.

=== Cause 4 of 5: legacy QueryFilter/ScanFilter/KeyConditions arrive at an unimplemented-feature refusal instead of validation ===
AWS: an illegal combination of KeyConditions / QueryFilter / ScanFilter arguments is ValidationException (HTTP 400).
Emulator: the request reaches a scaffold guard and answers NotImplemented (HTTP 501) before the argument rules are evaluated.
Why: 501 is not a client error — SDK retry and error-mapping treat it as a server fault, and the caller never learns its arguments were illegal.
Fix: run legacy-argument validation before the feature-gate check in the Query/Scan handlers.
Traces: legacy-queryfilter-scanfilter-keyconditions-argument-rules.

=== Cause 5 of 5: BatchGetItem/Query requests that mix a projection with legacy parameters are accepted ===
AWS: a BatchGetItem that mixes ProjectionExpression with AttributesToGet on one table, and a Query that mixes KeyConditions with expression parameters, are both refused with ValidationException.
Emulator: the call succeeds with HTTP 200 and an empty body.
Why: a request AWS always refuses silently "works" here, so the defect only surfaces in production.
Fix: the mixed-parameter check in the BatchGetItem and Query handlers.
Traces: null-false-batchget-mixed-projection-and-query-keyconditions-with-expressions.

## Evidence
Cause 1 — ddb-sfa-legacy-not-null-with-a-value-list-rejected: expected 'One or more parameter values were invalid: Invalid number of argument(s) for the NOT_NULL ComparisonOperator', got 'One or more parameter values were invalid: NOT_NULL takes 0 value(s) in AttributeValueList, got 1'. ddb-ss-scan-filter-mixed-with-filter-expression-rejected: raised 'Can not use both expression and non-expression parameters in the same request: Non-expression parameters: {ScanFilter} Expression parameters: {FilterExpression}', expected 'Non-expression parameters: {ScanFilter, ConditionalOperator}'. scan-query-conditional-operator-needs-two-filter-conditions-rejected: got 'ConditionalOperator can only be used together with ScanFilter'. conditionaloperator-enum-error-precedes-nesting-depth-in-legacy-expected / scan-filter-conditional-operator-document-value-equality: got "1 validation error detected: Value 'XOR' at 'conditionalOperator' failed to satisfy constraint: Member must satisfy enum value set: [AND, OR]" where AWS reports the ConditionalOperator defect.

Cause 2 — ddb-sfa-legacy-scan-filter-not-null-and-null-match-the-expression-functions: ScanFilter NOT_CONTAINS returned ('user-001','user-003','website-001','website-002','website-003') and the exists-plus-negated-contains expression ('user-001','user-003'); both must be ('user-001','user-003'). scan-filter-ne-and-not-contains-on-missing-attribute: NOT_CONTAINS on a missing attribute returned ['no-attr'], expected no items.

Cause 3 — scan-filter-binary-attribute-unsigned-byte-order: ScanFilter CONTAINS 0x00 returned [], expected only the value holding it as a subsequence.

Cause 4 — legacy-queryfilter-scanfilter-keyconditions-argument-rules: QueryFilter/EQ: expected ValidationException, got NotImplemented.

Cause 5 — null-false-batchget-mixed-projection-and-query-keyconditions-with-expressions: expected the call to be refused, but it succeeded: HTTPStatusCode 200, content-length 2.

## Steps to Reproduce
1. Create a table and seed items where some carry attribute 'status' and some do not.
2. ddb.scan(TableName=t, ScanFilter={'status': {'ComparisonOperator': 'NOT_CONTAINS', 'AttributeValueList': [{'S': 'x'}]}}) and compare the item set with ddb.scan(FilterExpression='attribute_exists(#s) AND NOT contains(#s, :x)').
3. ddb.scan(TableName=t, ScanFilter={'a': {'ComparisonOperator': 'NOT_NULL', 'AttributeValueList': [{'S': 'v'}]}}) and read Error.Message.
4. ddb.scan(TableName=t, ScanFilter={...}, ConditionalOperator='AND', FilterExpression='...') and read the non-expression parameter list in Error.Message.
5. ddb.batch_get_item(RequestItems={t: {'Keys': [...], 'ProjectionExpression': 'a', 'AttributesToGet': ['a']}}) — expect ValidationException, observe HTTP 200.
6. ddb.query(TableName=t, KeyConditions={...}, QueryFilter={...}) — expect ValidationException, observe NotImplemented 501.
Then replay the dynamodb checks named in trace_ids.
