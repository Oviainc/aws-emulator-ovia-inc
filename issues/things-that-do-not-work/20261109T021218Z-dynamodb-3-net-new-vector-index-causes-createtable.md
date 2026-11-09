Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.create-table-vector-index-validation-rejected dynamodb.put-item-vector-write-validation-and-search-projection dynamodb.vector-write-request-bytes-floor-and-per-dimension-cost
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.create-table-vector-index-validation-rejected|_error#17|AssertionError|raise AssertionError(f'expected the call to be refused, but it succeeded: {result!r}')
Issue-Key: dynamodb.put-item-vector-write-validation-and-search-projection|run#24|AssertionError|assert message == expected, f'empty-string SearchSchema key message was {message!r}'
Issue-Key: dynamodb.vector-write-request-bytes-floor-and-per-dimension-cost|run#16|AssertionError|assert table_wcu == 17.0, f'table WCU for this write was {table_wcu!r}, expected 17.0'
Issue-Commit: 28a8a61701b6d4c7fe3b061a21d38872ad20994f

# dynamodb: 3 net-new vector-index causes — CreateTable accepts more than 5 vector indexes, the undeclared-SearchSchema refusal is emulator-worded, and a vector write's table CapacityUnits omits the per-index cost

> _Note: this issue was identified against the code base at commit `28a8a61701b6d4c7fe3b061a21d38872ad20994f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
Three net-new root causes in the DynamoDB vector-index surface. Each must be closed before submitting.

Cause 1 of 3: CreateTable does not enforce the per-table vector-index count limit.
AWS: Smithy: CreateTable input carries VectorIndexes -> VectorIndexList of com.amazonaws.dynamodb#VectorIndex (IndexName required, @length 3..255, @pattern ^[a-zA-Z0-9_.-]+$; VectorAttribute required; Dimensions required @range min 1; DistanceFunction required). The model states no per-table count limit, so recorded behavior only: AWS refuses a CreateTable carrying more vector indexes than the per-table limit with HTTP 400 ValidationException and the message "One or more parameter values were invalid: VectorIndex count exceeds the per-table limit of 5" — the sentence is fixed text and the trailing number is the per-table limit in force (5 by default) — and the table is never created, so a following DescribeTable answers ResourceNotFoundException "Requested resource not found: Table: <TableName> not found" (the table name comes from the request).
Emulator: the same CreateTable with six VectorIndexes answers HTTP 200 with a full TableDescription (TableStatus CREATING) that lists all six indexes, each with IndexStatus CREATING and its own IndexArn; nothing is refused.
Why: a client that over-provisions vector indexes gets a table AWS would never have built, so code written against the emulator ships a table shape that fails at deploy time against AWS, and the negative path it must handle (ValidationException + the table absent) is never exercised. The sibling Dimensions rule IS enforced (Dimensions 4097 is refused with "One or more parameter values were invalid: Number of dimensions must be between 1 and 4096 inclusive."), so only the count rule is missing; the check's third case — two VectorIndexes on the same vector attribute with different Dimensions, which AWS refuses with "One or more parameter values were invalid: Conflicting attribute definition for '<attr>'. All VectorIndexes on the same vector attribute must use the same dimensions." — was never reached and should be verified too.
Fix: CreateTable's request validation in the dynamodb service crate, in the vector-index validation path that already carries the Dimensions range check.
Failing check ids: dynamodb.create-table-vector-index-validation-rejected.

Cause 2 of 3: CreateTable's refusal for a SearchSchema element naming an undeclared attribute is emulator-authored.
AWS: Smithy: VectorIndex.SearchSchema -> SearchSchema (@length min 1) of SearchSchemaElement{AttributeName required @length 0..65535, SearchSchemaElementType required enum HASH|INLINE_FILTER}. The empty string is within the modeled length, so the refusal is a service rule, not a shape constraint — recorded behavior only: AWS refuses a CreateTable whose SearchSchema names any attribute absent from AttributeDefinitions (the empty string being one such name) with HTTP 400 ValidationException and the message "One or more parameter values were invalid: One element in SearchSchema is not defined in attribute definitions", which is entirely fixed text with no values interpolated from the request; the table is not created (DescribeTable then answers ResourceNotFoundException).
Emulator: ValidationException with the emulator's own sentence "Invalid SearchSchema for index <IndexName>: no AttributeDefinition was provided for search schema attribute <AttributeName>" — observed as 'Invalid SearchSchema for index idx: no AttributeDefinition was provided for search schema attribute ' — i.e. a different fixed text that additionally interpolates the index name and the offending attribute name.
Why: a client matching on AWS's message (log scrapers, error-classification code, tests) does not recognise the refusal, and the message leaks a wording AWS never emits.
Fix: the message construction for CreateTable's vector-index SearchSchema validation.
Failing check ids: dynamodb.put-item-vector-write-validation-and-search-projection.

Cause 3 of 3: a vector write's table-level ConsumedCapacity.Table.CapacityUnits omits the vector-index write cost.
AWS: Smithy: ConsumedCapacity.CapacityUnits is "The total number of capacity units consumed by the operation"; ConsumedCapacity.Table is the throughput consumed on the table; ConsumedCapacity.VectorIndexes is a VectorIndexesCapacityMap whose entries carry VectorWriteRequestBytes for writes. The exact arithmetic is not modeled, so recorded behavior only: for a PutItem with ReturnConsumedCapacity=INDEXES on a table with two vector indexes (one 4-dimension, one 4096-dimension) AWS reports VectorWriteRequestBytes 1024.0 for the small index (the per-write floor) and 16397.0 for the large one (dimensions*4 plus fixed overhead), and ConsumedCapacity.Table.CapacityUnits = 17.0 — the per-index vector bytes are rolled into the table's write units on top of the base item write.
Emulator: the per-index VectorWriteRequestBytes values agree (1024.0 and 16397.0), but ConsumedCapacity.Table.CapacityUnits is 9.0 instead of 17.0. Hypothesis: the aggregate counts only part of the per-index vector bytes (or none of the large index's) when folding them into table write units.
Why: metering is the billing surface — a client sizing on-demand/provisioned capacity or asserting on consumed capacity reads roughly half the write cost of a vector write, so capacity planning and any cost assertion built on the emulator is wrong. The check's follow-on claim — that a second write changing only one of the two vector attributes meters only that index and omits the untouched one from ConsumedCapacity.VectorIndexes — was never reached and should be verified with the same fix.
Fix: PutItem's consumed-capacity accounting for vector indexes (the aggregation of per-index VectorWriteRequestBytes into ConsumedCapacity.Table / ConsumedCapacity.CapacityUnits).
Failing check ids: dynamodb.vector-write-request-bytes-floor-and-per-dimension-cost.

Related but not covering these: 20261108T092824Z-dynamodb-5-net-new-causes-transactwriteitems-refuses-placeholders.md (vector-index UpdateTable/SearchVectors response divergence) and 20261029T180026Z-dynamodb-global-tables-replica-auto-scaling-indexed-query.md (vector operations answering 501) describe other vector causes; the three above are distinct and unowned.

## Evidence
Cause 1 — expected (AWS): CreateTable with six VectorIndexes -> HTTP 400 ValidationException, message "One or more parameter values were invalid: VectorIndex count exceeds the per-table limit of 5"; DescribeTable on that name -> ResourceNotFoundException "Requested resource not found: Table: <name> not found".
Actual (emulator): HTTP 200. AssertionError: expected the call to be refused, but it succeeded: {'TableDescription': {'TableName': 'sim2real-behav-ddbvecval-toomany-...', 'TableStatus': 'CREATING', ..., 'VectorIndexes': [{'IndexName': 'idx0', 'Dimensions': 8, 'DistanceFunction': 'COSINE', 'IndexStatus': 'CREATING', ...}, {'IndexName': 'idx1', ... 'IndexStatus': 'CREAT…  (all six indexes present)

Cause 2 — expected (AWS) message: 'One or more parameter values were invalid: One element in SearchSchema is not defined in attribute definitions'.
Actual (emulator) message: 'Invalid SearchSchema for index idx: no AttributeDefinition was provided for search schema attribute '  (error code ValidationException in both cases).

Cause 3 — expected (AWS): PutItem(ReturnConsumedCapacity='INDEXES') on a 2-vector-index table returns ConsumedCapacity.VectorIndexes['idx-small'].VectorWriteRequestBytes == 1024.0, ['idx-large'].VectorWriteRequestBytes == 16397.0 and ConsumedCapacity.Table.CapacityUnits == 17.0.
Actual (emulator): the two VectorWriteRequestBytes match, but AssertionError: table WCU for this write was 9.0, expected 17.0.

## Steps to Reproduce
Cause 1: ddb.create_table(TableName=T, KeySchema=[{pk,HASH}], AttributeDefinitions=[{pk,S}], BillingMode='PAY_PER_REQUEST', VectorIndexes=[six entries idx0..idx5, each VectorAttribute embN, Dimensions 8, DistanceFunction COSINE, Projection KEYS_ONLY]) -> emulator returns 200 with all six indexes; AWS refuses with ValidationException and never creates T (DescribeTable -> ResourceNotFoundException).

Cause 2: ddb.create_table(... AttributeDefinitions=[{pk,S}] only, VectorIndexes=[{IndexName:'idx', VectorAttribute:{AttributeName:'emb'}, Dimensions:4, DistanceFunction:'COSINE', Projection:{ProjectionType:'ALL'}, SearchSchema:[{AttributeName:'', SearchSchemaElementType:'INLINE_FILTER'}]}]) -> compare the ValidationException message against 'One or more parameter values were invalid: One element in SearchSchema is not defined in attribute definitions'.

Cause 3: create a PAY_PER_REQUEST table with VectorIndexes idx-small (attr emb_small, Dimensions 4) and idx-large (attr emb_large, Dimensions 4096); wait until the table and both indexes are ACTIVE; ddb.put_item(Item={'pk':{'S':'v1'},'emb_small':{'L':[{'N':'0.5'}]*4},'emb_large':{'L':[{'N':'0.5'}]*4096}}, ReturnConsumedCapacity='INDEXES') and read ConsumedCapacity.Table.CapacityUnits (9.0 observed, 17.0 expected). Then re-put changing only emb_small and check that ConsumedCapacity.VectorIndexes contains idx-small at 1024.0 and omits idx-large.
