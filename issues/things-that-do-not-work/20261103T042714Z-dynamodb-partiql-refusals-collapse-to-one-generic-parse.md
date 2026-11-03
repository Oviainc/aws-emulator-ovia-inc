Issue-Severity: HIGH
Issue-Commit: a2c0c19d1874b6d8f510c899cc1a51333f7d4b07

# DynamoDB PartiQL refusals collapse to one generic parse message and a successful BatchExecuteStatement slot omits TableName (3 causes)

> _Note: this issue was identified against the code base at commit `a2c0c19d1874b6d8f510c899cc1a51333f7d4b07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Three net-new root causes in the PartiQL surface (ExecuteStatement / BatchExecuteStatement / ExecuteTransaction). Open issue 20261029T180026Z-dynamodb-partiql-resource-policy-arn-addressing-crc32-header.md and 20261103T022510Z-dynamodb-expression-and-partiql-diagnostics-refusals-carry-emulator.md touch neighbouring ground; the suite did not mark any of the checks below as owned by them, so they are filed here and those issues should be closed together with this one if a fixer finds the same code path.

=== Cause 1 of 3: every PartiQL statement defect is reported with ONE invented sentence ===
AWS: Smithy models ExecuteStatement.Statement as PartiQLStatement with @length min 1 max 8192 and classifies statement defects as ValidationException (HTTP 400, service-common error). AWS's message is a fixed sentence per defect CLASS with the offending detail substituted from the statement: a parse defect is "Statement wasn't well formed, can't be processed: <reason>" where <reason> is the parser's own phrase for the position (recorded examples: "Unexpected token after expression", "Expected data manipulation", "Expected identifier for simple path"); an operand/arity defect is "Operator or function requires a document path; operator or function: <name>"; an unknown function is "Unrecognized function: <name> at <line>:<col>:<len>"; a FROM-clause arity defect is "A path may contain at most 2 components in the FROM clause"; inside ExecuteTransaction the parse defect is prefixed by the slot, "Validation failed in TransactStatements[<i>]: ...". Only the bracketed parts come from the request; the rest is fixed text.
Emulator: every one of those classes answers the single sentence "Invalid Statement: statement wasn't well formed, could not be parsed" (ValidationException), with no reason, no operator/function name, no position and no TransactStatements[i] prefix. For the BatchExecuteStatement slot error the Code is right (ValidationError in the per-slot BatchStatementResponse.Error) but the Message is that same generic sentence.
Why: a client cannot tell a syntax error from an unknown function from a wrong-arity function, cannot show the caller where the statement broke, and any branching or log-scraping on the message fails.
Fix: the PartiQL parser/validator behind ExecuteStatement, BatchExecuteStatement and ExecuteTransaction must carry a defect class plus the offending token/name/position into the ValidationException renderer, instead of one catch-all string.
Traces: batch-execute-statement-malformed-slot-siblings-execute, execute-statement-extra-token-after-complete-statement-rejected, execute-statement-function-requires-document-path-or-operand-count-rejected, execute-statement-malformed-literal-rejected, execute-statement-missing-clause-rejected, execute-statement-quoting-rules, execute-statement-undefined-function-and-projection-function-rejected, execute-transaction-malformed-statement-rejects-whole-call, partiql-from-clause-path-arity-rejected, partiql-unknown-index-qualifier-rejected.

=== Cause 2 of 3: non-parse PartiQL refusals are emulator-authored prose ===
AWS: a parameter-count mismatch, a write statement addressed through an index qualifier, an N literal over the 38-significant-digit domain and a transaction read statement that is not a single-item equality are each ValidationException with AWS's own fixed sentence — for the index-qualified write, "FROM clause may only contain a single table name in data manipulation statements at <line>:<col>:<len>".
Emulator: these answer hand-written sentences instead: 'Invalid Statement: the number of parameters in the request and in the statement do not match', 'This operation is not supported on an index', '1 validation error detected: Attempting to store more than 38 significant digits in a Number', and 'a read statement in a transaction must address one item of the table: its WHERE clause states an equality on every key attribute, and its source is the table rather than an index'. The last is emulator-authored explanatory prose, not an AWS message at all.
Why: the message is a documented part of the API contract; clients and SDK error-mapping layers that match on it break, and the leaked prose reveals the emulator.
Fix: the same ValidationException renderer in the PartiQL handlers; each refusal class needs AWS's sentence with only its variable parts (statement position, parameter counts, index name) interpolated.
Traces: execute-statement-parameter-count-and-key-type-mismatch-rejected, execute-statement-number-precision-in-update-set-and-key-equality, partiql-write-through-index-qualifier-rejected, execute-transaction-index-select-top-level-validation-not-cancellation.

=== Cause 3 of 3: a successful BatchExecuteStatement slot does not echo TableName ===
AWS: a BatchStatementResponse for a statement that executed carries TableName (the table the statement addressed) alongside Item where applicable, so a caller can correlate slots back to tables. Recorded behavior for the exact slot shape.
Emulator: the successful UPDATE slot comes back as only {'Item': {...}} with no TableName member.
Why: a caller that batched statements over several tables cannot attribute a slot's result to its table.
Fix: the BatchExecuteStatement response serializer / per-slot result assembly.
Traces: batch-execute-statement-returning-update-vs-delete-rejected.

## Evidence
Cause 1 — batch-execute-statement-malformed-slot-siblings-execute: AssertionError: expected message "Statement wasn't well formed, can't be processed: Unexpected token after expression", got "Invalid Statement: statement wasn't well formed, could not be parsed". execute-statement-quoting-rules: single-quoted table name: expected "Statement wasn't well formed, can't be processed: Expected identifier for simple path", got the same generic sentence. execute-statement-undefined-function-and-projection-function-rejected: expected 'Unrecognized function: made_up_function at 1:100:16', got the generic sentence. execute-transaction-malformed-statement-rejects-whole-call: expected "Validation failed in TransactStatements[0]: Statement wasn't well formed, can't be processed: Unexpected token after expression", got the generic sentence. partiql-from-clause-path-arity-rejected / partiql-unknown-index-qualifier-rejected: expected 'A path may contain at most 2 components in the FROM clause', got the generic sentence.

Cause 2 — execute-statement-function-requires-document-path-or-operand-count-rejected: expected 'Operator or function requires a document path; operator or function: attribute_exists'. partiql-write-through-index-qualifier-rejected: expected 'FROM clause may only contain a single table name in data manipulation statements at 1:13:29', got 'This operation is not supported on an index'. execute-statement-number-precision-in-update-set-and-key-equality: got '1 validation error detected: Attempting to store more than 38 significant digits in a Number'. execute-transaction-index-select-top-level-validation-not-cancellation: got the emulator-authored sentence 'a read statement in a transaction must address one item of the table: ...'.

Cause 3 — batch-execute-statement-returning-update-vs-delete-rejected: AssertionError: expected the successful UPDATE slot to echo TableName='<table>', got {'Item': {'sk': {'S': 's1'}, 'name': {'S': 'Ada'}, 'qty': {'N': '10'}, 'pk': {'S': 'p1'}}}.

## Steps to Reproduce
1. Create a PAY_PER_REQUEST table with HASH pk / RANGE sk and wait for ACTIVE.
2. ddb.batch_execute_statement(Statements=[{'Statement': 'INSRT INTO "<t>" VALUE {\'pk\':\'a\',\'sk\':\'s1\'}'}, ...two well-formed INSERTs...]) and read Responses[0]['Error']['Message'] — expect "Statement wasn't well formed, can't be processed: Unexpected token after expression", observe "Invalid Statement: statement wasn't well formed, could not be parsed".
3. ddb.execute_statement(Statement='SELECT * FROM "<t>" WHERE made_up_function(pk)') — expect 'Unrecognized function: made_up_function at <l>:<c>:<n>'.
4. ddb.execute_transaction(TransactStatements=[{'Statement': '<malformed>'}]) — expect the 'Validation failed in TransactStatements[0]: ...' prefix.
5. Run an UPDATE ... RETURNING ALL OLD * through batch_execute_statement and check the successful slot for TableName.
Then replay the dynamodb checks named in trace_ids.
