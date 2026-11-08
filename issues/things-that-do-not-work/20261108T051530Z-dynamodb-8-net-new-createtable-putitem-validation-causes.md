Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.create-table-gsi-key-schema-hash-after-range-rejected dynamodb.create-table-invalid-key-schema-fails dynamodb.ddb-gsi-create-table-with-an-undeclared-index-key-rejected dynamodb.vvd-key-schema-attribute-without-definition-rejected dynamodb.create-table-vector-index-validation-rejected dynamodb.put-item-vector-of-wrong-dimension-rejected dynamodb.put-item-vector-write-validation-and-search-projection dynamodb.vector-write-request-bytes-floor-and-per-dimension-cost dynamodb.provisioned-throughput-on-pay-per-request
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.create-table-gsi-key-schema-hash-after-range-rejected|run#5|AssertionError|assert (code, msg) == ("ValidationException", _HASH_AFTER_RANGE_MESSAGE), (
Issue-Key: dynamodb.create-table-invalid-key-schema-fails|run#14|AssertionError|assert message == expected_msg, f"expected message {expected_msg!r}, got {message!r}"
Issue-Key: dynamodb.ddb-gsi-create-table-with-an-undeclared-index-key-rejected|run#8|AssertionError|assert message == expected_msg, (
Issue-Key: dynamodb.vvd-key-schema-attribute-without-definition-rejected|run#8|AssertionError|assert message == expected_msg, f"expected message {expected_msg!r}, got {message!r}"
Issue-Key: dynamodb.create-table-vector-index-validation-rejected|run#13|AssertionError|assert message == expected, f'over-max Dimensions message was {message!r}'
Issue-Key: dynamodb.put-item-vector-of-wrong-dimension-rejected|run#15|AssertionError|assert err.get('Message') == expected, f"unexpected message {err.get('Message')!r}"
Issue-Key: dynamodb.put-item-vector-write-validation-and-search-projection|run#11|AssertionError|assert message == expected, f'non-numeric list element message was {message!r}'
Issue-Key: dynamodb.vector-write-request-bytes-floor-and-per-dimension-cost|run#14|AssertionError|assert small_bytes == 1024.0, (
Issue-Key: dynamodb.provisioned-throughput-on-pay-per-request|run#12|AssertionError|assert err.get('Message') == expected, f"expected message {expected!r}, got {err.get('Message')!r}"
Issue-Commit: 3778ba40189b37afffee3a954bf30487a96c0bc7

# dynamodb: 8 net-new CreateTable/PutItem validation causes — key-schema, undeclared-attribute, vector-index and PAY_PER_REQUEST refusal messages are emulator-authored, and VectorWriteRequestBytes ignores the per-write floor

> _Note: this issue was identified against the code base at commit `3778ba40189b37afffee3a954bf30487a96c0bc7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
Eight net-new root causes in DynamoDB's table-creation / item-write validation surface. Every one is a 400 ValidationException whose accept/reject decision is right but whose message text (or, for Cause 8, whose metered value) is the emulator's own invention rather than AWS's. Clients that branch on the message, and the SDK/CLI users who read it, see text real AWS never emits.

Cause 1 of 8: a GSI KeySchema with a HASH element after a RANGE element.
AWS: Smithy: CreateTable.GlobalSecondaryIndexes[].KeySchema -> KeySchema (list of KeySchemaElement{AttributeName, KeyType}); refusal is the service-common ValidationException, HTTP 400. AWS's message is entirely FIXED text with no variable part: "One or more parameter values were invalid: All HASH key attributes must precede RANGE key attributes in the KeySchema".
Emulator: 400 ValidationException with "Invalid KeySchema for index <indexName>: The third KeySchemaElement is not a RANGE key type" — a different rule (positional element typing) and a message whose only fixed part is the emulator's own wording; <indexName> is taken from the request.
Why: a client that matches AWS's HASH-precedes-RANGE text to tell a mis-ordered key schema from any other key-schema defect never matches, and the emulator's text names a positional rule AWS does not state.
Fix: CreateTable's key-schema validation (the GSI KeySchema branch).
Traces: dynamodb.create-table-gsi-key-schema-hash-after-range-rejected

Cause 2 of 8: a BASE-TABLE KeySchema element with no AttributeDefinitions entry.
AWS: Smithy: CreateTable.KeySchema (required) and CreateTable.AttributeDefinitions (required); ValidationException, HTTP 400. For the base table AWS answers the fixed text "Invalid KeySchema: Some index key attribute have no definition" — no names, counts or lists are interpolated.
Emulator: 400 ValidationException with the INDEX-shaped message instead: "One or more parameter values were invalid: Some index key attributes are not defined in AttributeDefinitions. Keys: [<key names>], AttributeDefinitions: [<declared names>]" (recorded with Keys: [pk, sk], AttributeDefinitions: [pk] — those values come from the request, the template is the defect).
Why: AWS distinguishes the base-table case (short fixed text) from the index case (list-bearing text); the emulator collapses both onto the index form, so a client cannot tell which key schema was wrong from the message.
Fix: CreateTable's key-schema validation — the base-table KeySchema vs AttributeDefinitions cross-check must use its own message.
Traces: dynamodb.create-table-invalid-key-schema-fails

Cause 3 of 8: the index/key list ordering inside the "not defined in AttributeDefinitions" message.
AWS: ValidationException, HTTP 400, message template "One or more parameter values were invalid: Some index key attributes are not defined in AttributeDefinitions. Keys: [<names>], AttributeDefinitions: [<names>]". Fixed text is everything outside the two bracketed lists; the lists are values drawn from the request. Recorded behavior only for the ORDER of those lists: AWS does not serialise them in request order — for a six-index CreateTable whose AttributeDefinitions were declared [ContentId, TaskId, CreationTime, AssignedTo, Priority] AWS reports AttributeDefinitions: [CreationTime, AssignedTo, ContentId, TaskId, Priority]; for a two-element schema declared KeySchema [PK, SK] / AttributeDefinitions [PK, EXTRA] AWS reports Keys: [SK, PK], AttributeDefinitions: [EXTRA, PK]. Hypothesis: AWS iterates an internal unordered set keyed by attribute name. AWS also reports the FIRST invalid index's WHOLE key schema in Keys, not just the undefined attribute.
Emulator: emits both lists in request order (AttributeDefinitions: [ContentId, TaskId, CreationTime, AssignedTo, Priority]; Keys: [PK, SK], AttributeDefinitions: [PK, EXTRA]), so the message diverges from AWS for the same request.
Why: the two messages are byte-compared by tooling and by these checks; a client that reproduces AWS's text for a given template cannot match the emulator's.
Fix: CreateTable's AttributeDefinitions cross-check — the serializer that renders the Keys/AttributeDefinitions lists.
Traces: dynamodb.ddb-gsi-create-table-with-an-undeclared-index-key-rejected, dynamodb.vvd-key-schema-attribute-without-definition-rejected

Cause 4 of 8: VectorIndexes Dimensions out of range.
AWS: Smithy: CreateTable.VectorIndexes[].Dimensions; ValidationException, HTTP 400. Fixed text: "One or more parameter values were invalid: Number of dimensions must be between 1 and 4096 inclusive." (note the trailing period; no variable part).
Emulator: 400 ValidationException with "One or more parameter values were invalid: Dimensions must be between 1 and 4096" — different wording, no trailing period.
Why: a client keying on AWS's dimension-limit sentence never matches.
Fix: CreateTable's VectorIndexes validation.
Traces: dynamodb.create-table-vector-index-validation-rejected

Cause 5 of 8: PutItem whose vector attribute has the wrong number of dimensions.
AWS: ValidationException, HTTP 400, template "One or more parameter values were invalid. Invalid size for parameter <attributeName>, Expected: <configuredDimensions>, Actual: <suppliedLength> IndexName: <indexName>" — fixed text is "One or more parameter values were invalid. Invalid size for parameter ", ", Expected: ", ", Actual: " and " IndexName: "; the attribute name, the index's configured Dimensions, the supplied list length and the index name are values from the request and the table's state.
Emulator: 400 ValidationException with "One or more parameter values were invalid: The number of dimensions in the vector attribute <attr> must match the number of dimensions configured for the vector index <index>: <dims>" — a wholly different template that also loses the Actual length.
Why: the caller cannot read the supplied-vs-expected lengths out of the emulator's message, which is the only place AWS reports them.
Fix: PutItem's vector-attribute validation.
Traces: dynamodb.put-item-vector-of-wrong-dimension-rejected

Cause 6 of 8: PutItem whose vector attribute holds a non-number element, or is not a list at all.
AWS: ValidationException, HTTP 400. For a bad element: "One or more parameter values were invalid. Invalid type for parameter <attr>[<index>], Expected: 32-bit floating point number, Actual: <type letter>. IndexName: <indexName>". For a non-list attribute: "One or more parameter values were invalid. Invalid type for parameter <attr>, Expected: 32-bit floating point number list IndexName: <indexName>". Fixed text is everything but the attribute name, the offending element index, the offending AttributeValue type letter and the index name.
Emulator: 400 ValidationException with "One or more parameter values were invalid: The vector attribute <attr> must hold only numbers, each a 32-bit IEEE-754 floating point value" — one generic sentence for both shapes, naming neither the offending element position nor its type.
Why: a client cannot locate which element of the vector was bad; AWS puts that position in the message and nowhere else.
Fix: PutItem's vector-attribute validation (element typing branch).
Traces: dynamodb.put-item-vector-write-validation-and-search-projection

Cause 7 of 8: VectorWriteRequestBytes does not apply the per-write floor.
AWS: recorded behavior only — a vector write is metered with a per-write floor, so a 4-dimension write reports VectorWriteRequestBytes 1024.0 (the floor), with larger dimensions charged per-dimension above it.
Emulator: reports VectorWriteRequestBytes 8233.0 for that same 4-dimension write — a raw per-dimension computation with no floor applied, and an order of magnitude above AWS.
Why: a client sizing vector-write cost (or asserting the documented floor) reads a wildly different number from the emulator.
Fix: the vector write-metering component that produces ConsumedCapacity's vector byte counters.
Traces: dynamodb.vector-write-request-bytes-floor-and-per-dimension-cost

Cause 8 of 8: ProvisionedThroughput supplied with BillingMode=PAY_PER_REQUEST.
AWS: Smithy: CreateTable.BillingMode (enum) and CreateTable.ProvisionedThroughput{ReadCapacityUnits, WriteCapacityUnits}; ValidationException, HTTP 400. Fixed text, naming the two members: "One or more parameter values were invalid: Neither ReadCapacityUnits nor WriteCapacityUnits can be specified when BillingMode is PAY_PER_REQUEST".
Emulator: 400 ValidationException with "One or more parameter values were invalid: ProvisionedThroughput cannot be specified when BillingMode is PAY_PER_REQUEST" — names the container member instead of the two capacity members AWS names.
Why: the message is the requirement; a client matching AWS's member-level wording never matches.
Fix: CreateTable's billing-mode/throughput validation.
Traces: dynamodb.provisioned-throughput-on-pay-per-request

No open issue covers these: the closest open dynamodb validation issues (20261029T180026Z-dynamodb-createtable-updatetable-putitem-validation-10-missing-or.md, 20261105T114920Z-dynamodb-10-net-new-key-attribute-value-validation.md) own other checks in the suite and none of the eight causes above.

## Evidence
Replay output, expected (real AWS) vs actual (emulator):

1. create-table-gsi-key-schema-hash-after-range-rejected — expected ('ValidationException', 'One or more parameter values were invalid: All HASH key attributes must precede RANGE key attributes in the KeySchema'); actual ('ValidationException', 'Invalid KeySchema for index by-a-b-c: The third KeySchemaElement is not a RANGE key type').

2. create-table-invalid-key-schema-fails — expected message 'Invalid KeySchema: Some index key attribute have no definition'; actual 'One or more parameter values were invalid: Some index key attributes are not defined in AttributeDefinitions. Keys: [pk, sk], AttributeDefinitions: [pk]'.

3. ddb-gsi-create-table-with-an-undeclared-index-key-rejected — expected '... Keys: [TaskStatus, CreationTime], AttributeDefinitions: [CreationTime, AssignedTo, ContentId, TaskId, Priority]'; actual '... Keys: [TaskStatus, CreationTime], AttributeDefinitions: [ContentId, TaskId, CreationTime, AssignedTo, Priority]'.
   vvd-key-schema-attribute-without-definition-rejected — expected '... Keys: [SK, PK], AttributeDefinitions: [EXTRA, PK]'; actual '... Keys: [PK, SK], AttributeDefinitions: [PK, EXTRA]'.

4. create-table-vector-index-validation-rejected — expected 'One or more parameter values were invalid: Number of dimensions must be between 1 and 4096 inclusive.'; actual 'One or more parameter values were invalid: Dimensions must be between 1 and 4096'.

5. put-item-vector-of-wrong-dimension-rejected — expected 'One or more parameter values were invalid. Invalid size for parameter embedding, Expected: 3, Actual: 2 IndexName: cos'; actual 'One or more parameter values were invalid: The number of dimensions in the vector attribute embedding must match the number of dimensions configured for the vector index cos: 3'.

6. put-item-vector-write-validation-and-search-projection — expected 'One or more parameter values were invalid. Invalid type for parameter emb[1], Expected: 32-bit floating point number, Actual: S. IndexName: cos'; actual 'One or more parameter values were invalid: The vector attribute emb must hold only numbers, each a 32-bit IEEE-754 floating point value'.

7. vector-write-request-bytes-floor-and-per-dimension-cost — AssertionError: 'a 4-dimension write is below the per-write floor; VectorWriteRequestBytes was 8233.0, expected the 1024-byte floor'.

8. provisioned-throughput-on-pay-per-request — expected 'One or more parameter values were invalid: Neither ReadCapacityUnits nor WriteCapacityUnits can be specified when BillingMode is PAY_PER_REQUEST'; actual 'One or more parameter values were invalid: ProvisionedThroughput cannot be specified when BillingMode is PAY_PER_REQUEST'.

HTTP status (400) and error code (ValidationException) agree with AWS in causes 1-6 and 8; only the message diverges. Cause 7 is a numeric divergence on a successful write.

## Steps to Reproduce
Replay the behavioral suite and re-run the listed dynamodb checks, or by hand against the emulator:
1. CreateTable with a GSI whose KeySchema is [HASH, RANGE, HASH] -> compare the ValidationException message to AWS's HASH-precedes-RANGE text.
2. CreateTable with KeySchema [pk HASH, sk RANGE] but AttributeDefinitions [pk] -> compare to 'Invalid KeySchema: Some index key attribute have no definition'.
3. CreateTable with a GSI keyed on an attribute absent from AttributeDefinitions -> compare the ordering of the Keys and AttributeDefinitions lists in the message.
4. CreateTable with VectorIndexes[0].Dimensions=4097 -> compare the dimension-range message.
5. On a table with a 3-dimension vector index, PutItem an embedding of length 2 -> compare the 'Invalid size for parameter' message.
6. PutItem an embedding list containing an S element, then a non-list embedding -> compare the 'Invalid type for parameter' messages.
7. PutItem a 4-dimension vector and read VectorWriteRequestBytes from ConsumedCapacity -> expect the 1024-byte floor.
8. CreateTable with BillingMode=PAY_PER_REQUEST and a ProvisionedThroughput block -> compare the message.
