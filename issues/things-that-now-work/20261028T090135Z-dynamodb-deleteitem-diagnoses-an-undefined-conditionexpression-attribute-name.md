Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.delete-item-condition-expression-invalid-attribute-name-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.delete-item-condition-expression-invalid-attribute-name-rejected|run#11|AssertionError|assert message == expected_msg, f"expected message {expected_msg!r}, got {message!r}"
Issue-Commit: 0ba4a6d0611b8ec8e3b3a33f291da7a865261cbb

# dynamodb: DeleteItem diagnoses an undefined ConditionExpression attribute name as an UNUSED ExpressionAttributeNames entry (wrong ValidationException message)

> _Note: this issue was identified against the code base at commit `0ba4a6d0611b8ec8e3b3a33f291da7a865261cbb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteItem

## Description
One net-new root cause.

Cause 1 of 1: when a ConditionExpression dereferences an expression-attribute-name token that ExpressionAttributeNames does not define, the emulator reports the unused-names diagnosis instead of the undefined-name diagnosis. Failing check: dynamodb.delete-item-condition-expression-invalid-attribute-name-rejected.

AWS: Smithy — DeleteItem input members ConditionExpression and ExpressionAttributeNames (ExpressionAttributeNameMap); the member documentation states "One or more substitution tokens for attribute names in an expression ... Use the # character in an expression to dereference an attribute name". ValidationException is not a modeled member of DeleteItem's error list (modeled: ConditionalCheckFailedException, ProvisionedThroughputExceededException, ResourceNotFoundException, ...); it is the service-common request-validation refusal, HTTP 400, error code ValidationException. For a ConditionExpression that uses a name token which ExpressionAttributeNames never defines, AWS refuses the call with ValidationException and the message "1 validation error detected: Invalid ConditionExpression: An expression attribute name used in the document path is not defined; attribute name: <token>". In that message everything up to and including "attribute name: " is fixed text; the only variable part is the undefined token taken verbatim from the request's ConditionExpression (here "#missing"). This diagnosis has precedence over the unused-names diagnosis: the same request also supplied a defined-but-never-referenced entry (#other), and AWS still reports the undefined name. AWS's other message, "Value provided in ExpressionAttributeNames unused in expressions: keys: {<tokens>}" (fixed text plus the set of unused tokens from the request), is reached only when every token the expression references IS defined.

Emulator: the call is refused with the right code, ValidationException, but the wrong message — 'Value provided in ExpressionAttributeNames unused in expressions: keys: {#other}'. It diagnoses the surviving unused entry and never reports that the expression dereferences an undefined token, i.e. the undefined-name resolution step is either missing or ordered after the unused-names check.

Why: a client parses or surfaces this message to locate the mistake in its expression. Told that #other is unused, the caller removes #other — the request still fails, because the real fault is the undefined #missing that the message never names. Library/SDK layers and test suites that assert on the documented validation text for undeclared name tokens also diverge, and a caller that distinguishes "declare the missing name" from "drop the surplus name" takes the wrong corrective action.

Fix: the ConditionExpression validation path for DeleteItem (services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/delete_item.rs and the shared expression-validation component it calls) must resolve every '#'-prefixed token in the expression against ExpressionAttributeNames and raise the undefined-name ValidationException with AWS's wording, and it must run that resolution BEFORE the unused-names check so the undefined token is what the caller is told about. The same precedence applies to the other operations that accept ConditionExpression/ExpressionAttributeNames.

Traces: dynamodb.delete-item-condition-expression-invalid-attribute-name-rejected (this check's positive control — the same DeleteItem with ExpressionAttributeNames={'#s':'status'} — is never reached, so only the message is known to diverge).

## Evidence
Request (after CreateTable pk:S, PAY_PER_REQUEST, and PutItem {pk:'item-1', status:'active'}):
  DeleteItem(TableName=<table>, Key={pk:{S:'item-1'}}, ConditionExpression='#missing = :v', ExpressionAttributeNames={'#other':'status'}, ExpressionAttributeValues={':v':{S:'active'}})

Expected (AWS): HTTP 400, Error.Code 'ValidationException', Error.Message '1 validation error detected: Invalid ConditionExpression: An expression attribute name used in the document path is not defined; attribute name: #missing' — fixed text through 'attribute name: ', variable part '#missing' copied from the request's ConditionExpression.

Actual (replay): Error.Code 'ValidationException' (matches), Error.Message 'Value provided in ExpressionAttributeNames unused in expressions: keys: {#other}' — the unused-entry diagnosis, whose variable part ({#other}) is the surplus name, not the undefined one.

Harness assertion: AssertionError: expected message '1 validation error detected: Invalid ConditionExpression: An expression attribute name used in the document path is not defined; attribute name: #missing', got 'Value provided in ExpressionAttributeNames unused in expressions: keys: {#other}'

## Steps to Reproduce
1. Build and boot the emulator and replay dynamodb.delete-item-condition-expression-invalid-attribute-name-rejected.
2. CreateTable(TableName=<t>, KeySchema=[{pk,HASH}], AttributeDefinitions=[{pk,S}], BillingMode=PAY_PER_REQUEST) and wait for the table_exists waiter.
3. PutItem(TableName=<t>, Item={pk:{S:'item-1'}, status:{S:'active'}}).
4. DeleteItem(TableName=<t>, Key={pk:{S:'item-1'}}, ConditionExpression='#missing = :v', ExpressionAttributeNames={'#other':'status'}, ExpressionAttributeValues={':v':{S:'active'}}).
5. Observe the ValidationException message names the unused entry #other instead of the undefined token #missing.
6. (AWS parity continues: the item must survive the refusal, and the same DeleteItem with ExpressionAttributeNames={'#s':'status'} and ConditionExpression='#s = :v' must succeed.)
