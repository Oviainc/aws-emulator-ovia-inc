Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.transact-unused-expression-attribute-name-on-condition-check-and-get-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|TransactGetItems|NotImplemented
Issue-Commit: b65c74910afb27e037dd1378191061f945f503fe

# DynamoDB TransactGetItems with a ProjectionExpression is a scaffold stub: projected transactional reads answer 501 NotImplemented instead of serving them (and instead of refusing an unused ExpressionAttributeNames entry with ValidationException)

> _Note: this issue was identified against the code base at commit `b65c74910afb27e037dd1378191061f945f503fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TransactGetItems

## Description
REOPENS 20261023T142648Z-dynamodb-transactgetitems-and-transactwriteitems-cap-validation-messages-diverge-from.md: its fix left the call refused. That issue's subject was the over-cap `transactItems` constraint message; the call this check makes is still refused outright, on the operation's ProjectionExpression branch, so the operation remains unusable for a projected transactional read. 1 cause.

Cause 1 of 1: a `Get` inside TransactGetItems that carries a ProjectionExpression is answered 501 NotImplemented, so neither the legal projected read nor the input defect AWS refuses ever reaches the operation's semantics.

AWS: Smithy: TransactGetItems input `TransactItems` is required and targets TransactGetItemList (@length min 1 max 100); each TransactGetItem carries `Get` with members `Key` (required), `TableName` (required, target TableArn, @length 1..1024), `ProjectionExpression` (target ProjectionExpression) and `ExpressionAttributeNames` (target ExpressionAttributeNameMap, documented as "One or more substitution tokens for attribute names in the ProjectionExpression parameter"). Output is `Responses` -> ItemResponseList whose ItemResponse carries only `Item`. The operation's modeled errors are InternalServerError (500), InvalidEndpointException (421), ProvisionedThroughputExceededException, RequestLimitExceeded, ResourceNotFoundException, ThrottlingException and TransactionCanceledException (400) — NotImplemented / HTTP 501 is not among them, and there is no modeled way for this operation to decline a request it parsed. So (a) a Get whose ProjectionExpression names an attribute through a declared placeholder is SERVED with HTTP 200 and Responses[i].Item restricted to the projected attributes, and (b) an ExpressionAttributeNames entry that the ProjectionExpression never references is refused with HTTP 400 ValidationException whose message is "Value provided in ExpressionAttributeNames unused in expressions: keys: {#n}" — the fixed text is "Value provided in ExpressionAttributeNames unused in expressions: keys: {...}" and the variable part is the set of unused placeholder keys taken from the request (`#n` in this check), exactly as the same rule is reported on the single-item and write paths.

Emulator: every TransactGetItems call whose Get carries a ProjectionExpression is refused with HTTP 501 and botocore surfaces `An error occurred (NotImplemented) when calling the TransactGetItems operation: operation TransactGetItems with ProjectionExpression is scaffolded but not implemented in this emulator`. Both the invalid form (ProjectionExpression="pk" with an unused ExpressionAttributeNames {"#n": "color"}, which AWS refuses with ValidationException) and the valid form (ProjectionExpression="#n" with ExpressionAttributeNames {"#n": "color"}, which AWS serves 200 with Item == {"color": {"S": "red"}}) hit the same stub. The TransactWriteItems half of the same check — the identical unused-name rule on a ConditionCheck — is answered correctly, so the divergence is isolated to the TransactGetItems projection branch and is not the shared unused-name validator.

Why: 501 is outside the operation's modeled error set, so every AWS SDK classifies it as a non-retryable protocol fault rather than a ValidationException or a cancellation; a client that reads a consistent, projected snapshot of several items in one transaction cannot do it at all, and a client that sends a malformed projection cannot distinguish its own bad request from a missing emulator feature.

Fix: the TransactGetItems handler (services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/transact_get_items.rs) — implement the Get.ProjectionExpression / ExpressionAttributeNames path against the projection evaluator, and hoist the unused-placeholder validation ahead of the feature gate so a malformed request is refused with ValidationException before the stub branch is reached. Related open issue 20261103T065250Z-dynamodb-legal-projection-reads-answered-501-notimplemented-validation.md covers the same 501 on GetItem/Query/Scan/BatchGetItem and the validation-ordering defect for a malformed TransactGetItems request; it does not carry this check or the TransactGetItems ProjectionExpression branch.

Traces: dynamodb.transact-unused-expression-attribute-name-on-condition-check-and-get-rejected (suite heading `### dynamodb`).

## Evidence
Replayed behavioral suite, service heading '### dynamodb', check [error] dynamodb.transact-unused-expression-attribute-name-on-condition-check-and-get-rejected. Verbatim tail of the failure:

  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TransactGetItems operation: operation TransactGetItems with ProjectionExpression is scaffolded but not implemented in this emulator

Expected (AWS) for the two TransactGetItems calls the check makes after its TransactWriteItems assertions have already passed:
- transact_get_items(TransactItems=[{"Get": {"TableName": T, "Key": {"pk": {"S": "seed"}}, "ProjectionExpression": "pk", "ExpressionAttributeNames": {"#n": "color"}}}]) -> HTTP 400, Error.Code "ValidationException", Error.Message "Value provided in ExpressionAttributeNames unused in expressions: keys: {#n}", and NO CancellationReasons in the response (it is a top-level validation failure, not a cancellation).
- transact_get_items(TransactItems=[{"Get": {"TableName": T, "Key": {"pk": {"S": "seed"}}, "ProjectionExpression": "#n", "ExpressionAttributeNames": {"#n": "color"}}}]) -> HTTP 200 with Responses == [{"Item": {"color": {"S": "red"}}}] for a seeded item {"pk": {"S": "seed"}, "color": {"S": "red"}}.
Actual: HTTP 501 NotImplemented with the scaffold message above for both.

Model: smithy_query(operation=TransactGetItems) -> TransactItems required, TransactGetItemList @length{min:1,max:100}; errors = InternalServerError(500), InvalidEndpointException(421), ProvisionedThroughputExceededException(400), RequestLimitExceeded(400), ResourceNotFoundException(400), ThrottlingException(400), TransactionCanceledException(400). smithy_query(shape=Get) -> members Key(required), TableName(required, TableArn, @length 1..1024), ProjectionExpression, ExpressionAttributeNames; smithy_query(shape=ItemResponse) -> single member Item.

The check is not marked ALREADY FILED and not marked OWNED by any open issue in the replay report.

## Steps to Reproduce
1. Boot the emulator and point botocore at its dynamodb endpoint.
2. CreateTable with KeySchema pk(HASH, S), BillingMode PAY_PER_REQUEST; wait for ACTIVE.
3. PutItem {"pk": {"S": "seed"}, "color": {"S": "red"}}.
4. Call TransactGetItems with one Get: TableName=<table>, Key={"pk":{"S":"seed"}}, ProjectionExpression="pk", ExpressionAttributeNames={"#n":"color"} -> observed 501 NotImplemented 'operation TransactGetItems with ProjectionExpression is scaffolded but not implemented in this emulator'; expected 400 ValidationException 'Value provided in ExpressionAttributeNames unused in expressions: keys: {#n}' with no CancellationReasons.
5. Call TransactGetItems with the same Get but ProjectionExpression="#n" -> observed the same 501; expected HTTP 200 with Responses == [{"Item": {"color": {"S": "red"}}}].
6. Re-replay the behavioral check dynamodb.transact-unused-expression-attribute-name-on-condition-check-and-get-rejected to confirm the fix (its TransactWriteItems/ConditionCheck assertions already pass).
