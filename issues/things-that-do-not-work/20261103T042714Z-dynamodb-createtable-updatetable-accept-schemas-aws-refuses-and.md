Issue-Severity: HIGH
Issue-Commit: a2c0c19d1874b6d8f510c899cc1a51333f7d4b07

# DynamoDB CreateTable/UpdateTable accept schemas AWS refuses and the refusals that fire use invented messages and the wrong error class (10 causes)

> _Note: this issue was identified against the code base at commit `a2c0c19d1874b6d8f510c899cc1a51333f7d4b07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
Ten net-new root causes in the table control plane. Open issues 20261029T180026Z-dynamodb-createtable-updatetable-putitem-validation-10-missing-or.md and 20261103T022510Z-dynamodb-control-plane-batch-transact-apis-legacy-parameters.md are adjacent; the suite did not mark any check below as owned by them.

=== Cause 1 of 10: creating a table that is still CREATING is reported as "already exists" ===
AWS: a second CreateTable for a name whose table is in CREATING is ResourceInUseException (modeled on CreateTable) with "Attempt to change a resource which is still in use: Table is being created: <tableName>". Only once the table is ACTIVE does AWS say the table already exists.
Emulator: every loser answers 'Table already exists: <tableName>'.
Why: a client that retries on the "being created" message (the documented transient case) treats a transient state as permanent and aborts.
Fix: the CreateTable handler must branch on the existing table's TableStatus when rejecting a duplicate name.
Traces: concurrent-create-table-single-winner, vvd-duplicate-table-name-rejected.

=== Cause 2 of 10: KeySchema / AttributeDefinitions refusals are emulator-authored and under-report the offending index ===
AWS: an index KeySchema whose elements are out of order is ValidationException "Invalid KeySchema for index <name>: The <ordinal> KeySchemaElement is not a RANGE key type" using AWS's ordinal wording; an index key attribute with no AttributeDefinition is "One or more parameter values were invalid: Some index key attributes are not defined in AttributeDefinitions. Keys: [<that index's WHOLE key schema, in schema order>], AttributeDefinitions: [<the declared names>]" — for the FIRST invalid index AWS lists the index's complete key schema, not only the missing attribute; and a table-level KeySchema attribute without a definition is "One or more parameter values were invalid: Some index key attributes are not defined in AttributeDefinitions. Keys: [...], AttributeDefinitions: [...]".
Emulator: it lists only the single missing attribute in Keys, and for the table-level case answers the hand-written 'Invalid KeySchema: Some index key attribute have no definition'.
Why: the caller cannot see which index key schema is incomplete, and exact-match handling fails.
Fix: the CreateTable/UpdateTable KeySchema validator and its message builder.
Traces: create-table-gsi-key-schema-hash-after-range-rejected, ddb-gsi-create-table-with-an-undeclared-index-key-rejected, vvd-key-schema-attribute-without-definition-rejected.

=== Cause 3 of 10: billing-mode/throughput refusals are worded wrongly ===
AWS: ProvisionedThroughput supplied with BillingMode=PAY_PER_REQUEST is "One or more parameter values were invalid: ProvisionedThroughput cannot be specified when BillingMode is PAY_PER_REQUEST"; the same clash on UpdateTable has AWS's own sentence naming the parameter it rejects.
Emulator: both answer 'One or more parameter values were invalid: Neither ReadCapacityUnits nor WriteCapacityUnits can be specified when BillingMode is PAY_PER_REQUEST'.
Why: the sentence names the wrong parameter, so a caller fixing its request is pointed at the capacity members rather than at ProvisionedThroughput.
Fix: the billing-mode/throughput consistency check in the CreateTable and UpdateTable handlers.
Traces: vvd-on-demand-table-with-provisioned-throughput-rejected, update-table-throughput-on-on-demand-table-and-zero-capacity-rejected.

=== Cause 4 of 10: a GSI with Projection INCLUDE and no NonKeyAttributes is accepted ===
AWS: ProjectionType=INCLUDE requires NonKeyAttributes; CreateTable without it is refused with ValidationException.
Emulator: CreateTable returns HTTP 200 with TableStatus CREATING and a GlobalSecondaryIndexes entry carrying Projection {'ProjectionType': 'INCLUDE'} and no NonKeyAttributes.
Why: a template or IaC definition AWS always rejects deploys here and the index projects nothing predictable.
Fix: Projection validation in the CreateTable index validator.
Traces: create-table-rejects-malformed-requests.

=== Cause 5 of 10: a Create and a Delete of a GSI paired in one UpdateTable is accepted ===
AWS: GlobalSecondaryIndexUpdates may carry at most one index operation per UpdateTable call; a Create paired with a Delete is refused.
Emulator: the call succeeds.
Why: index mutations AWS serialises are applied together, so the emulator's index state diverges from what the same calls produce on AWS.
Fix: the GlobalSecondaryIndexUpdates arity/combination check in the UpdateTable handler.
Traces: ddb-gsi-create-and-delete-paired-in-one-update-rejected.

=== Cause 6 of 10: UpdateTable accepts an AttributeDefinitions set that no longer covers the live index keys ===
AWS: UpdateTable refuses a change that would leave a live index key attribute without an AttributeDefinition.
Emulator: the call succeeds (HTTP 200, TableStatus UPDATING) and DescribeTable then reports AttributeDefinitions holding only 'pk'.
Why: the table is left in a state AWS cannot represent, so later index operations and exports behave differently.
Fix: AttributeDefinitions/index-key cross-validation in the UpdateTable handler.
Traces: update-table-gsi-attribute-definitions-track-live-keys.

=== Cause 7 of 10: deleting an index that does not exist answers the wrong error class ===
AWS: a GlobalSecondaryIndexUpdates Delete naming an index the table does not have is ValidationException; a duplicate index name on create is ValidationException "Attempting to create an index which already exists".
Emulator: the delete answers ResourceNotFoundException 'Requested resource not found: Index not found for table <t>: <index>'.
Why: the error class differs, so SDK retry/classification and caller branching differ.
Fix: the GlobalSecondaryIndexUpdates validator's error classification and messages in the UpdateTable handler.
Traces: update-table-delete-of-an-absent-gsi-rejected.

=== Cause 8 of 10: vector-index definitions are not validated on CreateTable ===
AWS: a VectorIndexes entry whose Dimensions exceeds the supported ceiling is refused with ValidationException.
Emulator: CreateTable succeeds and the TableDescription reports VectorIndexes with Dimensions 4097.
Why: an index AWS will never create exists here, so vector capacity and search behaviour cannot be reproduced.
Fix: vector-index validation in the CreateTable handler.
Traces: create-table-vector-index-validation-rejected.

=== Cause 9 of 10: a declared index never reaches ACTIVE, so warm-throughput defaults are unobservable ===
AWS: a newly created table's secondary index transitions to ACTIVE and DescribeTable reports its WarmThroughput defaults.
Emulator: the index never reports ACTIVE within the check's wait, so the defaults can never be read.
Why: any client that waits for an index to become ACTIVE before using it blocks forever.
Fix: index status transition in the table state machine / DescribeTable projection.
Traces: ddb-wf-warm-throughput-defaults-applied.

=== Cause 10 of 10: a BatchWriteItem request AWS refuses is accepted with an empty UnprocessedItems ===
AWS: the request shape exercised by this check is refused with ValidationException, so the message a customer sees names the concept it broke.
Emulator: the call returns HTTP 200 {'UnprocessedItems': {}}.
Why: invalid input is silently absorbed, so the defect never surfaces until the same code runs against AWS.
Fix: BatchWriteItem request-shape validation in its handler.
Traces: validation-messages-are-customer-intelligible.

## Evidence
Cause 1 — concurrent-create-table-single-winner: expected every loser to get 'Attempt to change a resource which is still in use: Table is being created: <t>', got ['Table already exists: <t>', ...x7]. vvd-duplicate-table-name-rejected: expected 'Attempt to change a resource which is still in use: Table is being created: <t>', got 'Table already exists: <t>'.

Cause 2 — create-table-gsi-key-schema-hash-after-range-rejected: a GSI KeySchema HASH, RANGE, HASH gave 'Invalid KeySchema for index by-a-b-c: The third KeySchemaElement is not a RANGE key type' (unexpected). ddb-gsi-create-table-with-an-undeclared-index-key-rejected: got 'Keys: [TaskStatus], AttributeDefinitions: [ContentId, TaskId, CreationTime, AssignedTo, Priority]', expected 'Keys: [TaskStatus, CreationTime], AttributeDefinitions: [CreationTime, AssignedTo, ContentId, TaskId, Priority]'. vvd-key-schema-attribute-without-definition-rejected: expected 'One or more parameter values were invalid: Some index key attributes are not defined in AttributeDefinitions. Keys: [SK, PK], AttributeDefinitions: [EXTRA, PK]', got 'Invalid KeySchema: Some index key attribute have no definition'.

Cause 3 — vvd-on-demand-table-with-provisioned-throughput-rejected: expected 'One or more parameter values were invalid: ProvisionedThroughput cannot be specified when BillingMode is PAY_PER_REQUEST', got '... Neither ReadCapacityUnits nor WriteCapacityUnits can be specified when BillingMode is PAY_PER_REQUEST'. update-table-throughput-on-on-demand-table-and-zero-capacity-rejected: same emulator sentence where AWS serves its own.

Cause 4 — create-table-rejects-malformed-requests: expected the call to be rejected, but it SUCCEEDED and returned TableDescription with GlobalSecondaryIndexes[0].Projection {'ProjectionType': 'INCLUDE'} and TableStatus 'CREATING'.

Cause 5 — ddb-gsi-create-and-delete-paired-in-one-update-rejected: the call SUCCEEDED; real AWS rejects it.

Cause 6 — update-table-gsi-attribute-definitions-track-live-keys: expected the call to be rejected, but it succeeded: TableStatus 'UPDATING', AttributeDefinitions [{'AttributeName':'pk','AttributeType':'S'}].

Cause 7 — update-table-delete-of-an-absent-gsi-rejected: deleting the absent GSI 'by-nothing' gave 'ResourceNotFoundException': 'Requested resource not found: Index not found for table <t>: by-nothing'.

Cause 8 — create-table-vector-index-validation-rejected: expected the call to be refused, but it succeeded: VectorIndexes [{'IndexName':'idx', 'Dimensions': 4097, 'DistanceFunction':'COSINE', ...}].

Cause 9 — ddb-wf-warm-throughput-defaults-applied: WorkflowIndex did not reach ACTIVE within 1s.

Cause 10 — validation-messages-are-customer-intelligible: real AWS rejects this call, but it SUCCEEDED and returned {'UnprocessedItems': {}} at HTTP 200.

## Steps to Reproduce
1. Fire 8 concurrent ddb.create_table calls for one name and collect every loser's Error.Message — expect 'Attempt to change a resource which is still in use: Table is being created: <t>'.
2. ddb.create_table with a GSI whose KeySchema is [HASH, RANGE, HASH] and read Error.Message.
3. ddb.create_table with a GSI whose HASH/RANGE attributes are not in AttributeDefinitions and read the Keys: [...] list — it must name that index's whole key schema.
4. ddb.create_table(BillingMode='PAY_PER_REQUEST', ProvisionedThroughput={...}) and read Error.Message.
5. ddb.create_table with a GSI Projection {'ProjectionType': 'INCLUDE'} and no NonKeyAttributes — expect ValidationException, observe 200.
6. ddb.update_table(GlobalSecondaryIndexUpdates=[{'Create': ...}, {'Delete': ...}]) — expect ValidationException, observe success.
7. ddb.update_table(GlobalSecondaryIndexUpdates=[{'Delete': {'IndexName': 'by-nothing'}}]) and read Error.Code — expect ValidationException.
8. ddb.create_table with VectorIndexes Dimensions=4097 — expect ValidationException, observe 200.
9. ddb.create_table with a secondary index, then poll describe_table until the index reports IndexStatus ACTIVE.
Then replay the dynamodb checks named in trace_ids.
