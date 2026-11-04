Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-condition-expression-contains-same-path-and-between-reversed-rejected
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-condition-expression-contains-same-path-and-between-reversed-rejected dynamodb-malformed-n-in-expression-value-rejected dynamodb-partiql-relational-operators-reject-complex-type-operands dynamodb-get-item-expression-attribute-name-numeric-value-rejected dynamodb-refarch-rekreup-stale-row-read-addressing dynamodb-trajwf-untyped-expression-values-rejected dynamodb-transact-get-items-top-level-vs-cancellation-error-class dynamodb-csvtraj-single-column-csv-hash-reused-as-range dynamodb-ddbkds-traj-wrong-region-table-arn dynamodb-rbp-traj-malformed-resource-arn-classes dynamodb-rbp-traj-stream-arn-has-no-table-policy
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|UpdateItem|4xx->4xx|$.__type
Issue-Key: dynamodb|PutItem|4xx->4xx|$.__type
Issue-Key: dynamodb|ExecuteStatement|4xx->4xx|$.__type
Issue-Key: dynamodb|GetItem|4xx->4xx|$.__type
Issue-Key: dynamodb|Scan|4xx->4xx|$.Message
Issue-Key: dynamodb|Scan|4xx->4xx|$.__type
Issue-Key: dynamodb|TransactGetItems|4xx->4xx|$.CancellationReasons
Issue-Key: dynamodb|TransactGetItems|4xx->4xx|$.Message
Issue-Key: dynamodb|TransactGetItems|4xx->4xx|$.__type
Issue-Key: dynamodb|CreateTable|4xx->4xx|$.__type
Issue-Key: dynamodb|DescribeKinesisStreamingDestination|4xx->4xx|$.__type
Issue-Key: dynamodb|GetResourcePolicy|4xx->4xx|$.__type
Issue-Related: 20261023T105249Z-dynamodb-resource-based-policy-lifecycle-diverges-policy-unreadable-conditioned.md
Issue-Related: 20261030T060227Z-dynamodb-resource-policy-refusals-name-the-wrong-resource.md
Issue-Related: 20261030T060227Z-dynamodb-transaction-and-partiql-responses-omit-capacity-table.md
Issue-Related: 20261030T060227Z-dynamodb-validationexception-messages-diverge-from-the-wording-aws.md
Issue-Related: 20261104T061030Z-dynamodb-refusal-text-is-emulator-authored-for-required.md
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# DynamoDB serves the wrong error class for malformed input and cancelled transactions

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateItem

## Description
Nine net-new root causes where the status is 400 on both sides but the emulator returns a different error class (__type) than AWS, so a client's error branch takes the wrong path. Causes 1-3 share a mechanism (validation deferred until expression evaluation) and causes 4-5 another (the serializer's error class), but each operation's refusal is a separate fix. Related open issues exist (20261103T042714Z-dynamodb-expression-validation-runs-at-evaluation-time-so.md, 20261103T042714Z-dynamodb-resource-based-policies-stream-arns-are-refused.md); none of the nine steps below is owned by them per the replay report.

Cause 1 of 9 — UpdateItem: contains() whose first operand equals a later operand.
AWS: refused at validation time, 400 __type 'com.amazon.coral.validate#ValidationException', message '1 validation error detected: Invalid ConditionExpression: The first operand must be distinct from the remaining operands for this operator or function; operator: contains, first operand: [<path>]' — fixed text with the operator and the attribute path from the request.
Emulator: 400 __type 'com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException', message 'The conditional request failed' — the expression was evaluated instead of validated.
Why: a client treats a malformed expression as a lost race on a condition and retries forever instead of fixing the request.
Fix: UpdateItem's ConditionExpression validation, which must run before condition evaluation.
Traces: dynamodb-condition-expression-contains-same-path-and-between-reversed-rejected#task#4 (also #task#5, dynamodb-malformed-n-in-expression-value-rejected#task#6, #task#8).

Cause 2 of 9 — PutItem: an ExpressionAttributeValues N value that is not a number.
AWS: 400 ValidationException, message '1 validation error detected: The parameter cannot be converted to a numeric value: <value>' — fixed text with the offending literal from the request.
Emulator: 400 ConditionalCheckFailedException 'The conditional request failed'.
Why / Fix: as cause 1, in PutItem's expression-value coercion, which must reject before the condition is evaluated.
Traces: dynamodb-malformed-n-in-expression-value-rejected#task#4 (also #task#10).

Cause 3 of 9 — ExecuteStatement: a relational operator applied to a document-type operand.
AWS: 400 ValidationException, message 'Incorrect operand type for operator or function; operator or function: <op>, operand type: <typeCode>' — fixed text with the operator and the DynamoDB type code of the operand.
Emulator: 400 ConditionalCheckFailedException 'The conditional request failed'.
Why / Fix: as cause 1, in the PartiQL operand type-checking before evaluation.
Traces: dynamodb-partiql-relational-operators-reject-complex-type-operands#task#9.

Cause 4 of 9 — GetItem: a JSON number where ExpressionAttributeNames requires a string.
AWS: this is a deserialization failure, not a validation failure: 400 __type 'com.amazon.coral.service#SerializationException' with the member 'Message' (capital M) = 'NUMBER_VALUE cannot be converted to String' (fixed text).
Emulator: __type 'ValidationException' — unqualified, with no namespace — and no Message member at all.
Why: the client sees neither AWS's error class nor any message; SDK error mapping (and the non-retryable classification that goes with SerializationException) is wrong, and a bare 'ValidationException' is not the shape AWS's JSON protocol emits.
Fix: the JSON request deserializer — emit SerializationException with a Message member when a payload value has the wrong JSON type.
Traces: dynamodb-get-item-expression-attribute-name-numeric-value-rejected#task#4 (also dynamodb-refarch-rekreup-stale-row-read-addressing#task#4).

Cause 5 of 9 — Scan: an expression value supplied without its AttributeValue type wrapper.
AWS: 400 'com.amazon.coral.service#SerializationException' with Message 'Unexpected value type in payload' (fixed text).
Emulator: __type 'ValidationException', no Message.
Why / Fix: as cause 4, in the same deserializer path reached from Scan.
Traces: dynamodb-trajwf-untyped-expression-values-rejected#task#4.

Cause 6 of 9 — TransactGetItems: one item's key does not match the table schema.
AWS: Smithy TransactGetItems models TransactionCanceledException among its errors. A per-item defect cancels the transaction: 400 __type 'com.amazonaws.dynamodb.v20120810#TransactionCanceledException', Message 'Transaction cancelled, please refer cancellation reasons for specific reasons [<reason codes, comma separated>]', and a CancellationReasons list with one entry per requested item — 'None' for the items with no defect and {Code: ValidationError, Message: 'The provided key element does not match the schema'} for the offender.
Emulator: a flat ValidationException with no CancellationReasons and no Message.
Why: the client cannot tell which item of the transaction was at fault, which is the entire purpose of CancellationReasons.
Fix: TransactGetItems' error path — per-item defects must be collected into a TransactionCanceledException rather than raised as a top-level validation error.
Traces: dynamodb-transact-get-items-top-level-vs-cancellation-error-class#task#7.

Cause 7 of 9 — CreateTable: an index key attribute with no matching AttributeDefinition is reported as a name collision.
AWS: 400 ValidationException 'Invalid KeySchema: Some index key attribute have no definition' (fixed text) — schema validation precedes the existence check.
Emulator: 400 'com.amazonaws.dynamodb.v20120810#ResourceInUseException' with 'Table already exists: <tableName>'.
Why: the client is told to pick another table name when the real defect is its key schema; a retry with a new name fails the same way on AWS.
Fix: CreateTable handler ordering — validate the request shape before testing for an existing table.
Traces: dynamodb-csvtraj-single-column-csv-hash-reused-as-range#task#2.

Cause 8 of 9 — DescribeKinesisStreamingDestination: a table ARN from another region is reported as a missing table.
AWS: 400 ValidationException with message "1 validation error detected: Invalid AWS region in '<arn>'" — fixed text with the ARN from the request.
Emulator: 400 ResourceNotFoundException 'Requested resource not found: Table: <name> not found' — it strips the ARN to a name and looks it up.
Why: a cross-region addressing bug looks like a missing resource, so the client creates the table instead of fixing its endpoint/ARN.
Fix: the ARN parser used by the Kinesis-destination handlers — validate the region component before resolving the table.
Traces: dynamodb-ddbkds-traj-wrong-region-table-arn#task#4.

Cause 9 of 9 — GetResourcePolicy: a stream ARN is refused as a malformed table ARN.
AWS: a stream ARN is a legal resource ARN shape; when no such stream exists the answer is 400 'com.amazonaws.dynamodb.v20120810#ResourceNotFoundException' with 'Requested resource not found: Stream: <streamLabel> not found for Table: <tableName>' — fixed text with the stream label and table name parsed out of the ARN.
Emulator: __type 'ValidationException' (again unqualified) with 'Invalid TableArn: Invalid ResourceArn provided as input <arn>'.
Why: the client cannot distinguish 'this ARN is not an ARN' from 'that stream does not exist'.
Fix: the resource-policy ARN parser — accept the stream ARN class and route to the not-found path.
Traces: dynamodb-rbp-traj-malformed-resource-arn-classes#task#6 (also dynamodb-rbp-traj-stream-arn-has-no-table-policy#task#5).

## Evidence
Replay of simparity-wire-aws-v1. Status 400 on both sides in every cause; __type (and where noted the message/members) diverged:
1. UpdateItem dynamodb-condition-expression-contains-same-path-and-between-reversed-rejected#task#4: com.amazon.coral.validate#ValidationException + '1 validation error detected: Invalid ConditionExpression: The first operand must be distinct from the remaining operands for this operator or function; operator: contains, first operand: [<path>]' -> com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException + 'The conditional request failed' (4 steps).
2. PutItem dynamodb-malformed-n-in-expression-value-rejected#task#4: ValidationException + '1 validation error detected: The parameter cannot be converted to a numeric value: <value>' -> ConditionalCheckFailedException + 'The conditional request failed' (2 steps).
3. ExecuteStatement dynamodb-partiql-relational-operators-reject-complex-type-operands#task#9: ValidationException + 'Incorrect operand type for operator or function; operator or function: <, operand type: L' -> ConditionalCheckFailedException + 'The conditional request failed'.
4. GetItem dynamodb-get-item-expression-attribute-name-numeric-value-rejected#task#4: com.amazon.coral.service#SerializationException + $.Message 'NUMBER_VALUE cannot be converted to String' -> __type 'ValidationException', $.Message absent (2 steps).
5. Scan dynamodb-trajwf-untyped-expression-values-rejected#task#4: SerializationException + $.Message 'Unexpected value type in payload' -> 'ValidationException', $.Message absent.
6. TransactGetItems dynamodb-transact-get-items-top-level-vs-cancellation-error-class#task#7: TransactionCanceledException + $.Message 'Transaction cancelled, please refer cancellation reasons for specific reasons [None, ValidationError]' + $.CancellationReasons [{Code: None},{Code: ValidationError, Message: 'The provided key element does not match the schema'}] -> 'ValidationException', no Message, no CancellationReasons.
7. CreateTable dynamodb-csvtraj-single-column-csv-hash-reused-as-range#task#2: ValidationException + 'Invalid KeySchema: Some index key attribute have no definition' -> ResourceInUseException + 'Table already exists: <name>'.
8. DescribeKinesisStreamingDestination dynamodb-ddbkds-traj-wrong-region-table-arn#task#4: ValidationException + "1 validation error detected: Invalid AWS region in '<arn>'" -> ResourceNotFoundException + 'Requested resource not found: Table: <name> not found'.
9. GetResourcePolicy dynamodb-rbp-traj-malformed-resource-arn-classes#task#6: ResourceNotFoundException + 'Requested resource not found: Stream: <label> not found for Table: <name>' -> 'ValidationException' + 'Invalid TableArn: Invalid ResourceArn provided as input <arn>' (2 steps).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Representative: replay fixture dynamodb-condition-expression-contains-same-path-and-between-reversed-rejected, step task#4 — UpdateItem with contains(#a, #a); the emulator answers ConditionalCheckFailedException instead of ValidationException.
3. For each remaining cause replay the fixture#step in trace_ids and compare $.__type first, then the message/members: malformed N in an expression value, a PartiQL relational operator on an L operand, a JSON number in ExpressionAttributeNames, an untyped expression value, a schema-mismatched TransactGetItems key, an undefined index key attribute on CreateTable, a cross-region table ARN on DescribeKinesisStreamingDestination, and a stream ARN on GetResourcePolicy.
