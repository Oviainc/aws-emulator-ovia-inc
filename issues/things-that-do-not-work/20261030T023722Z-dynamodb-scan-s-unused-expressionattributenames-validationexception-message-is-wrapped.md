Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.ddb-sfa-unused-expression-attribute-name-in-scan-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.ddb-sfa-unused-expression-attribute-name-in-scan-rejected|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: a1e58af128a9f30f24b7da24f66fe92e314560f8

# Scan's unused-ExpressionAttributeNames ValidationException message is wrapped in the "1 validation error detected: " constraint envelope AWS does not use

> _Note: this issue was identified against the code base at commit `a1e58af128a9f30f24b7da24f66fe92e314560f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Scan

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: a Scan whose `ExpressionAttributeNames` defines a token no expression member references is correctly refused with HTTP 400 `ValidationException`, but the emulator emits the refusal message through its generic constraint-violation envelope, prefixing the AWS sentence with `1 validation error detected: `.

AWS: DynamoDB refuses a request that defines a substitution token none of its expressions use with a 400 `ValidationException` whose message is the bare sentence `Value provided in ExpressionAttributeNames unused in expressions: keys: {<keys>}`. Fixed text: `Value provided in ExpressionAttributeNames unused in expressions: keys: {` … `}`; the variable part is the set of unused alias keys taken from the request's `ExpressionAttributeNames` (for the replayed request, the single unreferenced alias). There is NO `<N> validation error(s) detected: ` envelope on this message: that envelope is what AWS's Smithy/Coral boundary validator puts in front of MODELLED input-constraint violations (`Value '…' at '<member>' failed to satisfy constraint: …`), whereas this rule is enforced by the DynamoDB service itself after the boundary validator has already passed the request. Smithy: `Scan` models `ExpressionAttributeNames` only as `ExpressionAttributeNameMap` with no constraint that could produce a boundary violation, and `Scan`'s modeled error list carries no `ValidationException` at all (errors: InternalServerError, InvalidEndpointException, ProvisionedThroughputExceededException, RequestLimitExceeded, ResourceNotFoundException, ThrottlingException) — so the model says nothing about this message and the wording above is recorded behavior only, as asserted by the replayed check.

Emulator: the same request is refused with error code `ValidationException` (correct) but message `1 validation error detected: Value provided in ExpressionAttributeNames unused in expressions: keys: {#b}` — the AWS sentence with the constraint envelope prepended. Replay output: `AssertionError: expected message 'Value provided in ExpressionAttributeNames unused in expressions: keys: {#b}', got '1 validation error detected: Value provided in ExpressionAttributeNames unused in expressions: keys: {#b}'`.

Why: a client that classifies or surfaces DynamoDB refusals by message text — error-message assertions in tests, log scrapers, SDK wrappers that strip the known AWS prefix only for genuine boundary-constraint violations, and humans reading the text — sees a sentence that cannot occur on real DynamoDB. It also mislabels the failure class: the envelope claims a modelled input-constraint violation was detected at the boundary, when the request is in fact well formed per the model and was refused by the service's own expression-substitution rule. Any emulator-wide reliance on the envelope to mean "boundary validator" is therefore unreliable here, and the same defect will show on every other service-level DynamoDB refusal routed through the same helper.

Fix: the Scan handler's unused-substitution check and the shared refusal-wording helper it goes through. The handler raises the refusal via `crate::state::Substitutions::check_all_referenced([...FilterExpression, ProjectionExpression...])` and lifts it with `lift()` into `crate::scaffolded::Scaffolded::validation(message)`; the envelope itself is added by the shared `validation_message`/`refused` pair in the service's state module, which formats `1 validation error detected: <clause>` / `<N> validation errors detected: …`. A service-enforced expression-substitution refusal must be emitted as the bare AWS sentence, not routed through the constraint-violation envelope; the envelope stays only for refusals of modelled input constraints. The surrounding behavior is already correct and must stay: 400 `ValidationException`, nothing written, and the control Scan answering identically before and after.

Traces: dynamodb.ddb-sfa-unused-expression-attribute-name-in-scan-rejected (the only failing check grounding this cause; the check's positive control Scan and its post-rejection control Scan both pass, so the divergence is isolated to the refusal message).

File/line citations for the fixer: services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/scan.rs lines 390-401 (the `check_all_referenced` call) and 948-961 (`lift` -> `Scaffolded::validation`); services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/state.rs lines 470-530 (the `<N> validation error(s) detected: ` envelope) and 620-630 (`invalid_member`/`refused`); services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/scaffolded.rs lines 316-332 (`Scaffolded::validation`, which adds no prefix of its own). Not covered by any open issue: the suite marked this check neither ALREADY FILED nor OWNED, and the nearby open dynamodb issues 20261029T180026Z-dynamodb-createtable-updatetable-putitem-validation-10-missing-or.md (CreateTable/UpdateTable/PutItem validation) and 20261024T065715Z-dynamodb-iam-sts-dynamodb-data-plane-reads-are-never-authorized-identity.md (which owns dynamodb.validation-messages-name-the-concept-without-implementation-leakage) address different operations and a different cause.

## Evidence
Check: dynamodb.ddb-sfa-unused-expression-attribute-name-in-scan-rejected [mismatch]

Request replayed (after CreateTable with KeySchema id(S) HASH + timestamp(N) RANGE, PAY_PER_REQUEST, and a BatchWriteItem seed):
  Scan TableName=<table> ConsistentRead=true
       FilterExpression="attribute_exists(#a)"
       ExpressionAttributeNames={"#a": "status", "#b": "category"}   <- #b is never referenced

Expected (AWS): HTTP 400, error code ValidationException,
  Message = "Value provided in ExpressionAttributeNames unused in expressions: keys: {#b}"
  (fixed text "Value provided in ExpressionAttributeNames unused in expressions: keys: {" + the unreferenced alias key(s) from the request + "}"; no envelope prefix)

Actual (emulator): HTTP 400, error code ValidationException (correct),
  Message = "1 validation error detected: Value provided in ExpressionAttributeNames unused in expressions: keys: {#b}"

Replay assertion:
  File "check_dynamodb_ddb_sfa_unused_expression_attribute_name_in_scan_rejected.py", line 35, in run
    assert message == expected
  AssertionError: expected message 'Value provided in ExpressionAttributeNames unused in expressions: keys: {#b}', got '1 validation error detected: Value provided in ExpressionAttributeNames unused in expressions: keys: {#b}'

Status and error code agree with AWS; only the message wording diverges, by the prepended constraint-violation envelope. The emulator source confirms the envelope is a shared formatter, not AWS text: services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/state.rs:525 `1 => Some(format!("1 validation error detected: {}", clauses[0]))` and :527 `"{n} validation errors detected: {}"`.

Model check (smithy_query operation=Scan): ExpressionAttributeNames -> ExpressionAttributeNameMap with no length/pattern constraint; Scan declares no ValidationException among its errors, so no boundary-constraint violation can be the source of this refusal — the wording is recorded behavior only.

## Steps to Reproduce
1. CreateTable: TableName=<t>, KeySchema=[id HASH (S), timestamp RANGE (N)], AttributeDefinitions for both, BillingMode=PAY_PER_REQUEST; wait for ACTIVE.
2. BatchWriteItem a few items, some of which carry a 'status' attribute.
3. Positive control (passes today): Scan TableName=<t>, ConsistentRead=true, FilterExpression="attribute_exists(#a)", ExpressionAttributeNames={"#a":"status"} -> 200 with the items that carry 'status'.
4. Scan TableName=<t>, ConsistentRead=true, FilterExpression="attribute_exists(#a)", ExpressionAttributeNames={"#a":"status","#b":"category"}.
5. Observe the refusal: code is ValidationException (correct) but Message is prefixed with "1 validation error detected: ". AWS returns the bare sentence "Value provided in ExpressionAttributeNames unused in expressions: keys: {#b}".
6. Re-run step 3 to confirm the rejected read changed nothing (it does, today).

Re-replay to confirm a fix: dynamodb.ddb-sfa-unused-expression-attribute-name-in-scan-rejected
