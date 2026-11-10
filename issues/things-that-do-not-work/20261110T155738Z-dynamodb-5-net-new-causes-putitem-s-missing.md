Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.batch-write-item-key-defects-message-diverges-from-put-item dynamodb.request-validation-precedes-missing-table-lookup-rejected dynamodb.transact-write-items-first-fatal-defect-reported-depends-on-defect-class dynamodb.transact-write-items-overlength-token-masks-other-violations-rejected dynamodb.restore-to-point-in-time-excludes-writes-after-restore-time
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.batch-write-item-key-defects-message-diverges-from-put-item|run#8|AssertionError|assert put_message == expected_put_message, (
Issue-Key: dynamodb.request-validation-precedes-missing-table-lookup-rejected|check#11|AssertionError|assert (code, msg) == want, f"{label} on the missing table gave {code!r}: {msg!r}, expected {want!r}"
Issue-Key: dynamodb.transact-write-items-first-fatal-defect-reported-depends-on-defect-class|run#25|AssertionError|assert err_b1['Message'] == condition_syntax_message, (
Issue-Key: dynamodb.transact-write-items-overlength-token-masks-other-violations-rejected|run#18|AssertionError|assert err2['Message'] == two_violation_message, f"expected {two_violation_message!r}, got {err2['Message']!r}"
Issue-Key: dynamodb.restore-to-point-in-time-excludes-writes-after-restore-time|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# dynamodb: 5 net-new causes — PutItem's missing-key message, the ProjectionExpression syntax-error token, two TransactWriteItems validation-aggregation defects, and a PITR restore that never becomes restorable

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem, GetItem, TransactWriteItems, RestoreTableToPointInTime

## Description
Five independent root causes in the DynamoDB data plane and PITR surface, all grounded in failing behavioral checks of the `dynamodb` service. Each must be closed before this issue is submitted.

Cause 1 of 5: PutItem's ValidationException message for an item missing the partition key is the wrong template.
AWS: PutItem validates the item against the table key schema and refuses a missing key with HTTP 400, Error.Code `ValidationException` and the message `One or more parameter values were invalid: Missing the key <keyName> in the item`. Fixed text: `One or more parameter values were invalid: Missing the key ` + ` in the item`; the only variable part is the key attribute name taken from the table's KeySchema (`pk` in this check). Smithy: PutItem.Item is `@required` (PutItemInputAttributeMap) and PutItem models no key-specific error, so the refusal class is the service-common ValidationException; the message template is AWS's documented/recorded wording for this defect class (recorded behavior for the exact text). The sibling defects on the same path use the same template with a different suffix: `Type mismatch for key <keyName> expected: <schemaType> actual: <givenType>`.
Emulator: returns `ValidationException` with the message `One of the required keys was not given a value` — AWS's wording for GetItem/DeleteItem/UpdateItem `Key` defects, not PutItem's item-level template. It names neither the key attribute nor the defect.
Why: a client that parses or matches on the message (and SDK/CLI users reading it) cannot tell WHICH key attribute of WHICH item was missing, and tooling keyed on AWS's `One or more parameter values were invalid: ...` prefix mis-routes the error. The divergence also erases the deliberate contrast with BatchWriteItem, which uses one fixed generic message (`The provided key element does not match the schema`) for every key defect.
Fix: PutItem's input validation / key-schema check in the dynamodb service crate — the message template used when an item omits a key attribute, which must be PutItem's item-level template rather than the Key-map template shared by the key-addressed operations.
Traces: dynamodb.batch-write-item-key-defects-message-diverges-from-put-item (the assertion fires on the first PutItem probe, before the BatchWriteItem half of the check is reached).

Cause 2 of 5: the ProjectionExpression parser reports the wrong offending token for a doubled path separator.
AWS: an invalid ProjectionExpression is refused with HTTP 400 `ValidationException` and the message `Invalid ProjectionExpression: Syntax error; token: "<token>", near: "<context>"`. For the expression `a..b` the offending token is the second `.` and the context is `..b`, i.e. `Invalid ProjectionExpression: Syntax error; token: ".", near: "..b"`. Fixed text: `Invalid ProjectionExpression: Syntax error; token: "`, `", near: "`, `"`; the variable parts are the first token the parser cannot accept and the surrounding slice of the submitted expression. Smithy: GetItem.ProjectionExpression is an unconstrained string, so the model states nothing about the message — recorded behavior only for the template. The same check shows the sibling templates AWS uses on this path are already matched (`Invalid ProjectionExpression: The expression can not be empty;`, `Invalid ProjectionExpression: Two document paths overlap with each other; ...`).
Emulator: answers `ValidationException` with `Invalid ProjectionExpression: Syntax error; token: "b", near: ".b"` — it skips past the unexpected separator and blames the following identifier, so both the token and the near-context are shifted one element to the right.
Why: the message is the only machine- and human-readable pointer to the position of the syntax error. A client correcting an expression programmatically (or a developer reading the CLI error) is directed at a valid identifier instead of the real defect, and a test asserting AWS's wording fails.
Fix: the expression parser's syntax-error reporting in the dynamodb service crate (the token/near pair it emits for ProjectionExpression, shared with the FilterExpression/UpdateExpression/ConditionExpression templates) — it must report the first token it could not consume, not the token after it.
Traces: dynamodb.request-validation-precedes-missing-table-lookup-rejected (the check also confirms the surrounding contract — validation precedes the missing-table lookup — is already honoured; only this one message diverges).

Cause 3 of 5: TransactWriteItems reports a later action's defect instead of the first-listed one, and prefixes the nesting-limit message with a validation-error count AWS does not use there.
AWS: TransactWriteItems validates its actions in list order, and for two actions each carrying one fatal non-aggregatable defect it reports the defect of the action listed FIRST. With a malformed ConditionExpression first and a 33-level-deep item second the refusal is HTTP 400 `ValidationException` with `Invalid ConditionExpression: Syntax error; token: "AND", near: "pk AND"` (fixed text `Invalid ConditionExpression: Syntax error; token: "`/`", near: "`/`"`, variable parts from the submitted expression). With the order reversed the refusal is `Nesting Levels have exceeded supported limits: Attributes in the item have nested levels beyond supported limit` — a wholly fixed sentence with NO `N validation error(s) detected: ` prefix, because it is not an input-constraint violation. Smithy: TransactWriteItems.TransactItems is `@required` with `@length(min:1,max:100)`; the operation's modeled errors are TransactionCanceledException / ValidationException-class service-common errors, and a malformed expression or an over-deep item is a top-level ValidationException, not a cancellation.
Emulator: for `[malformed condition, 33-level item]` it answers `ValidationException` with `1 validation error detected: Nesting Levels have exceeded supported limits: Attributes in the item have nested levels beyond supported limit` — it (a) reports the SECOND action's defect, ignoring list order, and (b) wraps a non-constraint message in the `1 validation error detected: ` prefix reserved for input-constraint violations.
Why: a client that submits a multi-action transaction is told to fix the wrong action; after fixing the reported item it re-submits and is refused again for the defect that was never reported, so the repair loop does not converge. The spurious prefix additionally breaks clients and tests that distinguish constraint-violation messages (`N validation error(s) detected: ...`) from semantic refusals.
Fix: TransactWriteItems' request validation in the dynamodb service crate — the order in which per-action fatal defects are evaluated (first listed action wins) and the message assembly that decides whether the `N validation error(s) detected: ` prefix applies.
Traces: dynamodb.transact-write-items-first-fatal-defect-reported-depends-on-defect-class.

Cause 4 of 5: TransactWriteItems' constraint-violation aggregation never detects an action naming none of Delete/Put/Update/ConditionCheck, so it reports one violation where AWS reports two.
AWS: input-constraint violations on TransactWriteItems are aggregated into one message, `N validation error(s) detected: ` followed by the violations joined with `; `. For a request carrying an empty action object and an invalid ReturnConsumedCapacity the refusal is HTTP 400 `ValidationException` with `2 validation errors detected: Value '' at 'transactItems' failed to satisfy constraint: TransactWriteRequest should contain Delete or Put or Update or ConditionCheck; Value 'BOGUS' at 'returnConsumedCapacity' failed to satisfy constraint: Member must satisfy enum value set: [INDEXES, TOTAL, NONE]`. Fixed text: the `N validation error(s) detected: ` frame, the `Value '<v>' at '<member>' failed to satisfy constraint: ` frame and each constraint sentence; the variable parts are the submitted value, the lowerCamel member path and (for the enum) the member's modeled value set. Smithy: the TransactWriteItem union-like member set is Delete/Put/Update/ConditionCheck and ReturnConsumedCapacity is the `ReturnConsumedCapacity` enum, so both violations are model-stated.
Emulator: answers `ValidationException` with only `1 validation error detected: Value 'BOGUS' at 'returnConsumedCapacity' failed to satisfy constraint: Member must satisfy enum value set: [INDEXES, TOTAL, NONE]` — the empty `transactItems` element is not counted or reported at all, and the count is 1 instead of 2. (The same check's first probe, where an over-36-character ClientRequestToken masks every other violation, already matches AWS; Smithy: ClientRequestToken `@length(min:1,max:36)`.)
Why: a client fixing the reported violation re-submits and is refused again for the unreported empty action; a client counting violations from the `N validation errors detected` prefix under-reports. Nothing tells the caller that an action object carrying no operation is invalid.
Fix: TransactWriteItems' constraint-violation collection in the dynamodb service crate — the empty/none-selected TransactWriteItem member must contribute a `Value '' at 'transactItems' failed to satisfy constraint: TransactWriteRequest should contain Delete or Put or Update or ConditionCheck` violation to the aggregate, with the count updated accordingly.
Traces: dynamodb.transact-write-items-overlength-token-masks-other-violations-rejected.

Cause 5 of 5: point-in-time recovery never becomes restorable, so RestoreTableToPointInTime with an explicit RestoreDateTime is unreachable.
AWS: after UpdateContinuousBackups enables PITR, DescribeContinuousBackups reports PointInTimeRecoveryStatus `ENABLED` and a LatestRestorableDateTime that ADVANCES with the server clock (trailing real time by roughly five minutes), so a recorded restore time eventually falls inside the restorable window and RestoreTableToPointInTime is accepted; the restored table then reaches TableStatus `ACTIVE` and contains exactly the data written at or before RestoreDateTime. Smithy: RestoreTableToPointInTime models InvalidRestoreTimeException and PointInTimeRecoveryUnavailableException for the not-yet-restorable cases, which is what the check retries on.
Emulator: the check never gets past the PITR progression — it exhausts the suite's 60 s budget (`Check timed out after 60.0s`) while polling, with no refusal and no completion. Hypothesis: LatestRestorableDateTime is reported as a fixed or stale value that never catches up to a restore time taken from the server's own Date header, so the check's 600 s `LatestRestorableDateTime catching up to the recorded restore time` poll can never be satisfied; the alternative is that RestoreTableToPointInTime keeps answering InvalidRestoreTimeException / PointInTimeRecoveryUnavailableException indefinitely. Either way nothing the client can observe ever advances.
Why: no client can exercise point-in-time restore at all — RestoreTableToPointInTime with an explicit RestoreDateTime is permanently unavailable, and a backup/restore runbook or disaster-recovery test built on PITR hangs instead of being refused.
Fix: the continuous-backups/PITR state in the dynamodb service crate — DescribeContinuousBackups' EarliestRestorableDateTime/LatestRestorableDateTime must advance with the server clock once PITR is ENABLED, and RestoreTableToPointInTime must accept a RestoreDateTime inside that window and drive the target table to ACTIVE carrying only the writes at or before it.
Traces: dynamodb.restore-to-point-in-time-excludes-writes-after-restore-time.

## Evidence
Cause 1 — dynamodb.batch-write-item-key-defects-message-diverges-from-put-item:
  expected: ValidationException / 'One or more parameter values were invalid: Missing the key pk in the item'
  actual:   ValidationException / 'One of the required keys was not given a value'
  (assertion: "PutItem missing partition key: expected message 'One or more parameter values were invalid: Missing the key pk in the item', got 'One of the required keys was not given a value'")

Cause 2 — dynamodb.request-validation-precedes-missing-table-lookup-rejected, GetItem with ProjectionExpression 'a..b' on a table that does not exist:
  expected: ('ValidationException', 'Invalid ProjectionExpression: Syntax error; token: ".", near: "..b"')
  actual:   ('ValidationException', 'Invalid ProjectionExpression: Syntax error; token: "b", near: ".b"')

Cause 3 — dynamodb.transact-write-items-first-fatal-defect-reported-depends-on-defect-class, TransactItems=[ConditionCheck with ConditionExpression 'attribute_exists(pk AND', Put with a 33-level-deep item]:
  expected: ValidationException / 'Invalid ConditionExpression: Syntax error; token: "AND", near: "pk AND"'
  actual:   ValidationException / '1 validation error detected: Nesting Levels have exceeded supported limits: Attributes in the item have nested levels beyond supported limit'
  (both the chosen action and the message framing diverge; AWS's nesting message carries no 'N validation error detected: ' prefix)

Cause 4 — dynamodb.transact-write-items-overlength-token-masks-other-violations-rejected, TransactItems=[{}, valid Put] with ReturnConsumedCapacity='BOGUS' and no ClientRequestToken:
  expected: ValidationException / "2 validation errors detected: Value '' at 'transactItems' failed to satisfy constraint: TransactWriteRequest should contain Delete or Put or Update or ConditionCheck; Value 'BOGUS' at 'returnConsumedCapacity' failed to satisfy constraint: Member must satisfy enum value set: [INDEXES, TOTAL, NONE]"
  actual:   ValidationException / "1 validation error detected: Value 'BOGUS' at 'returnConsumedCapacity' failed to satisfy constraint: Member must satisfy enum value set: [INDEXES, TOTAL, NONE]"

Cause 5 — dynamodb.restore-to-point-in-time-excludes-writes-after-restore-time:
  expected: PITR ENABLED, LatestRestorableDateTime advancing past the recorded restore time, RestoreTableToPointInTime accepted, target table ACTIVE containing item-a ('before-restore-time') and NOT item-b ('after-restore-time')
  actual:   'Check timed out after 60.0s' — the client never observes the window advance and never gets a restore accepted or refused terminally

## Steps to Reproduce
Cause 1: CreateTable with KeySchema [{pk, HASH}] / AttributeDefinitions [{pk, S}] / BillingMode PAY_PER_REQUEST; wait ACTIVE; call PutItem(TableName=table, Item={"other": {"S": "x"}}) and read Error.Code and Error.Message.

Cause 2: on a table name that does not exist, call GetItem(TableName=absent, Key={"pk":{"S":"k"}}, ProjectionExpression="a..b") and read Error.Message.

Cause 3: on an ACTIVE table with hash key pk, call TransactWriteItems(TransactItems=[{"ConditionCheck": {TableName: t, Key: {"pk": {"S": "cc-key"}}, ConditionExpression: "attribute_exists(pk AND"}}, {"Put": {TableName: t, Item: {"pk": {"S": "deep33"}, "blob": <map nested 33 levels>}}}]) and read Error.Message; then swap the two actions and read it again.

Cause 4: on the same table call TransactWriteItems(TransactItems=[{}, {"Put": {TableName: t, Item: {"pk": {"S": "x"}}}}], ReturnConsumedCapacity="BOGUS") with no ClientRequestToken and read Error.Message (expect two aggregated violations).

Cause 5: CreateTable PAY_PER_REQUEST, wait ACTIVE, UpdateContinuousBackups(PointInTimeRecoveryEnabled=True), poll DescribeContinuousBackups until PointInTimeRecoveryStatus==ENABLED, PutItem item-a, record the server Date header plus 5 s as the restore time, then poll DescribeContinuousBackups until LatestRestorableDateTime >= that time — it never does. (Replay the four listed message checks plus this one; all five are in the `dynamodb` suite.)
