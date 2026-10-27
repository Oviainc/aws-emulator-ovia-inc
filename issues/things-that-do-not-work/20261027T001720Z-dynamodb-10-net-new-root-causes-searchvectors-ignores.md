Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.deep-dynamodb-1-ddbvec-search-vector-dimension-mismatch-rejected dynamodb.search-vectors-dimension-mismatch-rejected dynamodb.deep-dynamodb-1-ddbvec-hash-schema-search-without-condition-rejected dynamodb.deep-dynamodb-1-ddbvec-include-projection-hides-unprojected-attributes dynamodb.deep-dynamodb-1-ddbvec-search-condition-filters-below-topk-with-projection dynamodb.deep-dynamodb-1-ddbvec-inline-filter-range-comparator-rejected dynamodb.deep-dynamodb-1-ddbvec-two-index-updates-in-one-call-rejected dynamodb.put-item-vector-of-wrong-dimension-rejected dynamodb.enable-kinesis-invalid-arn dynamodb.enable-kinesis-streaming-destination-already-active-rejected dynamodb.ttl-disable-within-modification-interval-rejected dynamodb.deploy-cfn-ttl-attribute-expires-item
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.deep-dynamodb-1-ddbvec-search-vector-dimension-mismatch-rejected|run#8|AssertionError|assert err.get('Message') == expected_msg, (
Issue-Key: dynamodb.search-vectors-dimension-mismatch-rejected|run#15|AssertionError|assert err.get('Message') == expected, f"the message must state both dimensions, got {err.get('Message')!r}"
Issue-Key: dynamodb.deep-dynamodb-1-ddbvec-hash-schema-search-without-condition-rejected|run#16|AssertionError|assert err.get('Message') == expected_msg, (
Issue-Key: dynamodb.deep-dynamodb-1-ddbvec-include-projection-hides-unprojected-attributes|run#15|AssertionError|assert set(r['Item']) == {'pk', 'tier', 'note'}, (
Issue-Key: dynamodb.deep-dynamodb-1-ddbvec-search-condition-filters-below-topk-with-projection|run#11|AssertionError|assert set(r['Item']) == {'pk', 'cat', 'note', 'extra'}, (
Issue-Key: dynamodb.deep-dynamodb-1-ddbvec-inline-filter-range-comparator-rejected|_error#13|AssertionError|raise AssertionError(f'{what}: the call was ACCEPTED; real AWS rejects it')
Issue-Key: dynamodb.deep-dynamodb-1-ddbvec-two-index-updates-in-one-call-rejected|run#12|AssertionError|assert err.get('Code') == 'LimitExceededException', (
Issue-Key: dynamodb.put-item-vector-of-wrong-dimension-rejected|_error_code#12|AssertionError|raise AssertionError(f'{what}: the call was ACCEPTED; real AWS rejects it')
Issue-Key: dynamodb.enable-kinesis-invalid-arn|run#24|AssertionError|assert final['DestinationStatus'] == 'ENABLE_FAILED', (
Issue-Key: dynamodb.enable-kinesis-streaming-destination-already-active-rejected|run#17|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: dynamodb.ttl-disable-within-modification-interval-rejected|_error#12|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: dynamodb|DescribeTimeToLive|ResourceNotFoundException
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# dynamodb: 10 net-new root causes — SearchVectors ignores vector-index projection and invents both of its ValidationException messages, UpdateTable VectorIndexUpdates is a 501 stub, vector-dimension and TTL-interval refusals are missing, a Kinesis destination for a nonexistent stream settles ACTIVE, and a CFN-deployed table with TTL never exists

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchVectors

## Description
Ten distinct net-new root causes in the `dynamodb` emulator, each grounded in a failing behavioral check that the replay report marked neither ALREADY FILED nor OWNED. The emulator is serving (164/458 checks pass, bootstrap_signal false, dynamodb 152/308), so these are fidelity divergences, not a bootstrap gap. Every cause below must be closed before this issue is submitted. Model citations are from services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json.

=== Cause 1 of 10: SearchVectors' search-vector/index dimension-mismatch refusal carries an emulator-authored message ===
AWS: Smithy: SearchVectors.SearchVector is @required (target SearchVectorList, @length 1-4096) and the index's dimension count is VectorIndexDescription.Dimensions (@range min 1). SearchVectors models no dimension error of its own (InternalServerError 500, RequestLimitExceeded 400, ResourceNotFoundException 400, ThrottlingException 400), so the mismatch arrives as DynamoDB's service-common ValidationException at HTTP 400. Recorded message template: `Input search vector dimension <n> does not match vector index dimension <m>` — the fixed text is `Input search vector dimension ` / ` does not match vector index dimension `, and the two variable parts are the length of the request's SearchVector list and the target index's configured Dimensions from service state.
Emulator: the call is refused, but the message is rewritten as `The number of dimensions in the search vector must match the number of dimensions configured for the vector index: provided <n>, configured <m>` (observed `provided 3, configured 4` on one index and `provided 1, configured 3` on another).
Why: a client that matches on AWS's message text — or a human reading a log — does not recognise the refusal, and the message is the only place the two dimensions are reported.
Fix: the SearchVectors handler's dimension validation and the ValidationException message it formats (the SearchVectors operation handler in the dynamodb service crate).
Traces: dynamodb.deep-dynamodb-1-ddbvec-search-vector-dimension-mismatch-rejected, dynamodb.search-vectors-dimension-mismatch-rejected.

=== Cause 2 of 10: SearchVectors' missing-SearchConditionExpression refusal carries an emulator-authored message ===
AWS: Smithy: SearchVectors.SearchConditionExpression is an optional String; the index's SearchSchema (VectorIndexDescription.SearchSchema, @length min 1) declares which attributes are HASH and which are inline filters. When the index's SearchSchema has a HASH key, the call without a SearchConditionExpression is refused with ValidationException at HTTP 400 and the message `SearchConditionExpression must be provided when SearchSchema has a HASH key` — entirely fixed text, no request values.
Emulator: the refusal happens but the message is `The value of each HASH attribute of the vector index search schema must be provided in the SearchConditionExpression`.
Why: same as cause 1 — the wording is a requirement, and this one is pure fixed text, so there is nothing environment-specific about the divergence.
Fix: the SearchVectors handler's SearchSchema/SearchConditionExpression validation message.
Traces: dynamodb.deep-dynamodb-1-ddbvec-hash-schema-search-without-condition-rejected.

=== Cause 3 of 10: SearchVectors ignores the vector index's Projection and returns attributes the index does not project, including the raw vector attribute ===
AWS: Smithy: VectorIndexDescription.Projection is documented as `Specifies attributes that are copied (projected) from the table into the vector index`. A read of a vector index therefore returns the table/index key attributes plus exactly the projected non-key attributes — an INCLUDE projection naming one attribute yields the keys plus that attribute and nothing else, and the vector attribute itself is not handed back as an item attribute.
Emulator: SearchVectors returns the item's full attribute set from the base table. On an index whose Projection is INCLUDE[note] over key `pk` and search-schema attribute `tier`, the returned item carried `['emb', 'note', 'pk', 'tier']` where `{pk, tier, note}` was owed; on a second index the returned item carried `['cat', 'emb', 'extra', 'note', 'pk']` where `{pk, cat, note, extra}` was owed. In both cases the embedding attribute `emb` and the unprojected attributes leak.
Why: the index projection is the contract that bounds what a vector search reads and costs. Clients that store large embeddings or non-public attributes in the base table and deliberately exclude them from an index get them back anyway — a payload-size and data-exposure divergence, and projection-based cost reasoning is meaningless.
Fix: the SearchVectors read path must filter each result through the index's Projection (and keep the vector attribute out of the returned item) rather than returning the base-table item — the SearchVectors handler and the vector-index read path in the service's table state.
Traces: dynamodb.deep-dynamodb-1-ddbvec-include-projection-hides-unprojected-attributes, dynamodb.deep-dynamodb-1-ddbvec-search-condition-filters-below-topk-with-projection.

=== Cause 4 of 10: a SearchConditionExpression applying a range comparator to an INLINE_FILTER search-schema attribute is accepted ===
AWS: recorded behavior only — the model constrains SearchConditionExpression only as a String, but the recording shows real AWS REFUSING a SearchConditionExpression that uses a range comparator (`price > :p`) against an attribute the index's SearchSchema declares as an inline filter. Hypothesis: the refusal class is DynamoDB's service-common ValidationException at HTTP 400, as for every other malformed expression; the recording establishes only that the call is refused, not its code or message.
Emulator: the call is ACCEPTED and returns HTTP 200 with search results.
Why: the emulator is more permissive than AWS on a negative class. A client develops against a query shape that production rejects, and the inline-filter/HASH distinction in the search schema has no observable effect.
Fix: SearchConditionExpression validation in the SearchVectors handler — resolve each referenced attribute against the index's SearchSchema and refuse comparators the schema role does not permit.
Traces: dynamodb.deep-dynamodb-1-ddbvec-inline-filter-range-comparator-rejected.

=== Cause 5 of 10: UpdateTable with VectorIndexUpdates answers 501 NotImplemented, so the per-call index-update limit is never enforced ===
AWS: Smithy: UpdateTableInput.VectorIndexUpdates is an optional list (target VectorIndexUpdateList, whose member VectorIndexUpdate carries Create/Delete actions). UpdateTable's modeled errors are InternalServerError (500), InvalidEndpointException (421), LimitExceededException (400), ResourceInUseException (400) and ResourceNotFoundException (400) — there is no NotImplemented error and no 501 in the operation's error set. Two Create actions in one UpdateTable call exceed the per-call index-update limit and are refused with LimitExceededException at HTTP 400.
Emulator: every UpdateTable carrying VectorIndexUpdates answers HTTP 501 with code `NotImplemented` and message `operation UpdateTable with VectorIndexUpdates is scaffolded but not implemented in this emulator` — both where AWS succeeds and where AWS refuses with LimitExceededException.
Why: vector indexes cannot be added to or removed from an existing table at all, and the negative case reports an unmodeled server-side 501 that SDK retry policies treat as retryable instead of the modeled 400 client error.
Fix: UpdateTable's VectorIndexUpdates branch plus the vector-index state it needs, with the per-call update-count validation ordered ahead of the not-implemented check so the limit refusal survives until the branch is implemented (the UpdateTable handler in the dynamodb service crate).
Traces: dynamodb.deep-dynamodb-1-ddbvec-two-index-updates-in-one-call-rejected.

=== Cause 6 of 10: PutItem accepts a vector attribute whose dimension does not match the index's configured Dimensions ===
AWS: Smithy: PutItem.Item is @required (PutItemInputAttributeMap) and PutItem models no dimension error, so the refusal arrives as DynamoDB's service-common ValidationException at HTTP 400; the index's dimension count is VectorIndexDescription.Dimensions. Recorded behavior: a PutItem whose vector attribute carries fewer components than the table's vector index declares is REFUSED — the write never lands. Hypothesis: the message names the provided and configured dimension counts, as the SearchVectors side does; the recording establishes only that the call is refused.
Emulator: the PutItem is ACCEPTED (HTTP 200) and an item whose embedding has the wrong dimension is stored.
Why: the table silently accumulates items the index cannot represent, so a subsequent vector search is answered from corrupt state instead of the write being refused at the door — the divergence is a durable data defect, not just a wrong status.
Fix: PutItem's item validation must check every vector attribute against the dimension of each vector index defined on the table before the write is applied (the PutItem handler and the shared item-validation path).
Traces: dynamodb.put-item-vector-of-wrong-dimension-rejected.

=== Cause 7 of 10: an EnableKinesisStreamingDestination whose StreamArn names a nonexistent Kinesis stream settles ACTIVE instead of ENABLE_FAILED ===
AWS: Smithy: DestinationStatus is an enum whose values are ENABLING, ACTIVE, DISABLING, DISABLED, ENABLE_FAILED and UPDATING — ENABLE_FAILED exists precisely for an enable that cannot be completed. EnableKinesisStreamingDestination's StreamArn is @required (@length 37-1024). A destination pointed at a stream that does not exist never reaches ACTIVE; it settles in ENABLE_FAILED.
Emulator: the destination is polled to a terminal state and reports `{'StreamArn': 'arn:aws:kinesis:<region>:<account>:stream/<nonexistent-stream>', 'DestinationStatus': 'ACTIVE'}` — the ENABLE_FAILED branch is unreachable, and the ARN is never resolved against Kinesis.
Why: a misconfigured pipeline reports healthy. A client that gates on DestinationStatus == ACTIVE believes change records are flowing to a stream that does not exist, and the status value that tells it otherwise can never be observed.
Fix: the EnableKinesisStreamingDestination handler must resolve StreamArn against the Kinesis service state and drive the destination to ENABLE_FAILED when it cannot be resolved, rather than unconditionally settling ACTIVE. Note: the OPEN issue 20261025T024002Z-dynamodb-enablekinesisstreamingdestination-stub.md lists this check id, but it is keyed to the operation's 501/NotImplemented refusal; the operation now answers with real behavior, so this status-machine defect is net-new work layered on that stub.
Traces: dynamodb.enable-kinesis-invalid-arn.

=== Cause 8 of 10: the already-enabled EnableKinesisStreamingDestination refusal carries an emulator-authored message ===
AW S: EnableKinesisStreamingDestination against a table whose destination is already ACTIVE is refused with the message `Table is not in a valid state to enable Kinesis Streaming Destination: EnableKinesisStreamingDestination must be DISABLED or ENABLE_FAILED to perform ENABLE operation.` — this is entirely fixed text; it names the required DestinationStatus values (both members of the DestinationStatus enum) and interpolates nothing from the request or from state.
Emulator: the refusal happens, but after the same `Table is not in a valid state to enable Kinesis Streaming Destination: ` prefix the emulator substitutes its own clause: `Streaming destination with streamArn: <stream-arn> is already enabled for table with tableName: <table-name>`, i.e. it replaces AWS's fixed statement of the legal source states with request/state values AWS does not put there.
Why: the half of the message that tells a client WHAT state transition is legal (`must be DISABLED or ENABLE_FAILED to perform ENABLE operation`) is gone, so the refusal is not actionable and message-matching clients break.
Fix: the already-enabled guard in the EnableKinesisStreamingDestination handler must emit AWS's constant clause.
Traces: dynamodb.enable-kinesis-streaming-destination-already-active-rejected.

=== Cause 9 of 10: UpdateTimeToLive accepts a disable issued inside the TTL modification interval ===
AWS: Smithy: UpdateTimeToLive.TimeToLiveSpecification is @required and its Enabled and AttributeName members are both @required; the operation's modeled errors are InternalServerError (500), InvalidEndpointException (421), LimitExceededException (400), ResourceInUseException (400) and ResourceNotFoundException (400). AWS rate-limits TTL setting changes: a second change to a table's TTL setting within the modification interval (documented as roughly one hour) is REFUSED rather than applied. Recorded behavior: enabling TTL and then immediately disabling it is refused; the check asserts a refusal and records its code and message. Hypothesis: the refusal class is DynamoDB's service-common ValidationException at HTTP 400 with a message stating that TTL has already been modified within the fixed interval — the recording establishes the refusal, and the exact wording must be taken from it.
Emulator: the disable SUCCEEDS with HTTP 200 and body `{"TimeToLiveSpecification": {"Enabled": false, "AttributeName": "<attr>"}}`, so the interval guard does not exist.
Why: the emulator is more permissive than AWS on a rate-limit class, so a client's TTL toggling logic that production refuses passes here; it also means the TTL setting has no modification history at all in service state.
Fix: the UpdateTimeToLive handler must record the time of the last TTL setting change on the table and refuse a further change inside the modification interval.
Traces: dynamodb.ttl-disable-within-modification-interval-rejected.

=== Cause 10 of 10: a CloudFormation-deployed AWS::DynamoDB::Table carrying TimeToLiveSpecification does not exist in DynamoDB afterwards ===
AWS: a stack declaring AWS::DynamoDB::Table reaches CREATE_COMPLETE only once the table exists; DescribeTimeToLive on the declared table then returns a TimeToLiveDescription (TimeToLiveStatus ENABLED on the attribute the template named) and items carrying a past timestamp in that attribute are expired.
Emulator: after the deploy reports success, dynamodb:DescribeTimeToLive on the stack's table answers ResourceNotFoundException at HTTP 400 with message `Requested resource not found: Table: <table-name> not found` — the table was never created, so the TTL half of the check is unreachable.
Why: no infrastructure-as-code path reaches a DynamoDB table, and the stack claims success for a resource that does not exist.
Fix: the CloudFormation resource provider for AWS::DynamoDB::Table must create the table in the DynamoDB service state (and apply TimeToLiveSpecification to it), failing the resource when it cannot. Note on existing issues: the OPEN issue 20261025T024002Z-dynamodb-10-net-new-root-causes-query-scan.md describes this family as its Cause 10 and the OPEN issue 20261025T015306Z-cloudformation-cannot-deploy-large-parts-of-the-resource.md as its Cause 7, but both are keyed to the dynamodb|DescribeTable|ResourceNotFoundException replay key and neither carries this check's key (the failing call here is DescribeTimeToLive), so the replay report marked this check neither ALREADY FILED nor OWNED.
Traces: dynamodb.deploy-cfn-ttl-attribute-expires-item.

## Evidence
Replayed suite simparity-behavioral-ga-prod-v1 against the booted emulator: 164/458 passed (35.8%), dynamodb 152/308. bootstrap_signal was false, so the emulator is serving and these are fidelity divergences. All twelve failing checks below were listed by the report WITHOUT an 'ALREADY FILED' or 'OWNED by' marker.

Cause 1 — dynamodb.deep-dynamodb-1-ddbvec-search-vector-dimension-mismatch-rejected [mismatch]:
  AssertionError: idx-euc, 3-d: expected message 'Input search vector dimension 3 does not match vector index dimension 4', got 'The number of dimensions in the search vector must match the number of dimensions configured for the vector index: provided 3, configured 4'.
  dynamodb.search-vectors-dimension-mismatch-rejected [mismatch]: AssertionError: the message must state both dimensions, got 'The number of dimensions in the search vector must match the number of dimensions configured for the vector index: provided 1, configured 3'.

Cause 2 — dynamodb.deep-dynamodb-1-ddbvec-hash-schema-search-without-condition-rejected [mismatch]:
  AssertionError: expected message 'SearchConditionExpression must be provided when SearchSchema has a HASH key', got 'The value of each HASH attribute of the vector index search schema must be provided in the SearchConditionExpression'.

Cause 3 — dynamodb.deep-dynamodb-1-ddbvec-include-projection-hides-unprojected-attributes [mismatch]:
  AssertionError: INCLUDE[note] index returned attributes ['emb', 'note', 'pk', 'tier']; expected pk, tier, note.
  dynamodb.deep-dynamodb-1-ddbvec-search-condition-filters-below-topk-with-projection [mismatch]: AssertionError: unprojected search item carried ['cat', 'emb', 'extra', 'note', 'pk'], expected pk, cat, note, extra.

Cause 4 — dynamodb.deep-dynamodb-1-ddbvec-inline-filter-range-comparator-rejected [mismatch]:
  AssertionError: SearchConditionExpression 'price > :p' on an INLINE_FILTER attribute: the call was ACCEPTED; real AWS rejects it (ddb.search_vectors(TableName=..., IndexName='idx-price', TopK=10, SearchVector=..., SearchConditionExpression='price > :p', ExpressionAttributeValues={':p': {'N': '15'}})).

Cause 5 — dynamodb.deep-dynamodb-1-ddbvec-two-index-updates-in-one-call-rejected [mismatch]:
  AssertionError: two Creates: expected LimitExceededException, got {'Message': 'operation UpdateTable with VectorIndexUpdates is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}.

Cause 6 — dynamodb.put-item-vector-of-wrong-dimension-rejected [mismatch]:
  AssertionError: a PutItem whose embedding has the wrong dimension: the call was ACCEPTED; real AWS rejects it (put_item(Item={'pk': {'S': 'bad'}, 'embedding': {'L': [{'N': '1'}, {'N': '0'}]}}) against an index of higher dimension).

Cause 7 — dynamodb.enable-kinesis-invalid-arn [mismatch]:
  AssertionError: a destination pointed at a nonexistent Kinesis stream must never reach ACTIVE: {'StreamArn': 'arn:aws:kinesis:us-west-2:000000000000:stream/no-such-stream-<suffix>', 'DestinationStatus': 'ACTIVE'}. Expected DestinationStatus 'ENABLE_FAILED'.

Cause 8 — dynamodb.enable-kinesis-streaming-destination-already-active-rejected [mismatch]:
  AssertionError: expected message 'Table is not in a valid state to enable Kinesis Streaming Destination: EnableKinesisStreamingDestination must be DISABLED or ENABLE_FAILED to perform ENABLE operation.', got 'Table is not in a valid state to enable Kinesis Streaming Destination: Streaming destination with streamArn: arn:aws:kinesis:us-west-2:000000000000:stream/<stream> is already enabled for table with tableName: <table>'.

Cause 9 — dynamodb.ttl-disable-within-modification-interval-rejected [mismatch]:
  AssertionError: expected the call to be rejected, but it succeeded: {'TimeToLiveSpecification': {'Enabled': False, 'AttributeName': 'expires_at'}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}} for update_time_to_live(TimeToLiveSpecification={'Enabled': False, 'AttributeName': 'expires_at'}) issued immediately after the enable.

Cause 10 — dynamodb.deploy-cfn-ttl-attribute-expires-item [error]:
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeTimeToLive operation: Requested resource not found: Table: <stack's table> not found. Expected: the CFN-declared table exists with TTL ENABLED on the named attribute.

## Steps to Reproduce
1. Build and boot the emulator, then replay the check ids in trace_ids.
2. Cause 1: CreateTable with a vector index of N dimensions, then SearchVectors with a SearchVector of M != N components -> observe the message text.
3. Cause 2: CreateTable with a vector index whose SearchSchema declares a HASH attribute, then SearchVectors with no SearchConditionExpression -> observe the message text.
4. Cause 3: CreateTable with a vector index whose Projection is INCLUDE of one non-key attribute, PutItem an item carrying the embedding plus extra attributes, then SearchVectors -> observe the returned item carries the embedding and the unprojected attributes.
5. Cause 4: on an index whose SearchSchema declares an inline-filter attribute, call SearchVectors with SearchConditionExpression '<inline-filter-attr> > :p' -> observe HTTP 200 instead of a refusal.
6. Cause 5: UpdateTable with VectorIndexUpdates containing two Create actions -> observe 501 NotImplemented instead of 400 LimitExceededException.
7. Cause 6: PutItem an item whose vector attribute has fewer components than the table's vector index declares -> observe HTTP 200 and the item stored.
8. Cause 7: EnableKinesisStreamingDestination with a StreamArn for a stream that does not exist, then poll DescribeKinesisStreamingDestination to a terminal status -> observe ACTIVE instead of ENABLE_FAILED.
9. Cause 8: EnableKinesisStreamingDestination twice for the same table and stream; read the second call's error message.
10. Cause 9: UpdateTimeToLive(Enabled=true) then immediately UpdateTimeToLive(Enabled=false) on the same table -> observe HTTP 200 instead of a refusal.
11. Cause 10: CreateStack with an AWS::DynamoDB::Table declaring TimeToLiveSpecification, wait for the deploy to report success, then call dynamodb:DescribeTimeToLive on the declared table name -> ResourceNotFoundException.
