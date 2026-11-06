Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.execute-statement-malformed-literal-rejected dynamodb.execute-statement-missing-clause-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.execute-statement-malformed-literal-rejected|_assert_rejected#10|AssertionError|assert message == expected_message, f"unexpected message {message!r}"
Issue-Key: dynamodb.execute-statement-missing-clause-rejected|_assert_rejected#10|AssertionError|assert message == expected_message, f"unexpected message {message!r}"
Issue-Commit: 7ff327d821bcd1f37fa31be7f3b9efbce7107294

# dynamodb: 2 net-new ExecuteStatement PartiQL rejection-message causes — a number split from its exponent and an UPDATE with no SET/REMOVE are refused with messages that are not AWS's

> _Note: this issue was identified against the code base at commit `7ff327d821bcd1f37fa31be7f3b9efbce7107294`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Two net-new root causes in the PartiQL statement parser/validator behind ExecuteStatement. Both refuse the statement with the right HTTP status (400) and the right error code (ValidationException) but with an Error.Message that is not the sentence AWS emits for that failure class. The error message is part of the contract, so each is a defect in its own right; they sit in two different code paths (the literal lexer's error text, and the clause-level validator's bespoke text), so they are two causes.

Cause 1 of 2: a numeric literal split from its exponent is reported as "Unexpected term" instead of AWS's empty-tail parse message.
AWS: ExecuteStatement(Statement="INSERT INTO \"<table>\" VALUE {'pk': 'py', 'n': 1e}") — a number literal whose exponent marker has no exponent digits — is refused HTTP 400 ValidationException with Message exactly "Statement wasn't well formed, can't be processed: " — i.e. the fixed text "Statement wasn't well formed, can't be processed: " and NOTHING after the colon-space for this lexical failure class (the same empty-tail form AWS uses for an unterminated string literal, which the emulator already gets right). Smithy: ExecuteStatement.Statement -> PartiQLStatement, @required, @length min 1 max 8192; ValidationException is the service-common 400 client error (ExecuteStatement's modeled errors are ConditionalCheckFailed/DuplicateItem/ItemCollectionSizeLimitExceeded/ProvisionedThroughputExceeded/RequestLimitExceeded/ResourceNotFound/Throttling/TransactionConflict/InternalServerError, none of which covers a parse failure). The exact sentence is recorded behavior only; the recording shows the empty tail. Nothing is written by the refused statement.
Emulator: same status and code, but Message "Statement wasn't well formed, can't be processed: Unexpected term" — the parser appends its own token-level diagnosis where AWS appends nothing. (The same check's later legs — an unclosed {...} item literal, expected tail "Expected RIGHT_CURLY", and an unclosed <<...>> set literal, expected tail "Expected RIGHT_DOUBLE_ANGLE_BRACKET" — are never reached because the check stops at this assertion, so they are unverified rather than known-good.)
Why: a client that branches on the parse-failure message (or asserts it in a test, or surfaces it to an end user) sees a diagnosis AWS never produces for this input, and cannot distinguish the empty-tail lexical class from the token-expectation class that AWS reserves for unclosed item/set literals.
Fix: the PartiQL lexer/parser error mapping used by the ExecuteStatement handler's validation (the step that turns a parse failure into the ValidationException message), in the dynamodb service.
Traces: dynamodb.execute-statement-malformed-literal-rejected.

Cause 2 of 2: an UPDATE statement with neither a SET nor a REMOVE clause is refused with a hand-written sentence instead of AWS's parse message.
AWS: ExecuteStatement(Statement="UPDATE \"<table>\" WHERE pk='<key>'") is refused HTTP 400 ValidationException with Message exactly "Statement wasn't well formed, can't be processed: Expected data manipulation" — all fixed text, no variable parts; AWS treats the missing SET/REMOVE as a parse failure of the statement, not as a semantic rule of its own. Recorded behavior only for the sentence; Smithy models no parse error for Statement. The rejected UPDATE must leave the item unchanged.
Emulator: same status and code, but Message "Invalid Statement: an UPDATE must state at least one SET or REMOVE clause" — a bespoke sentence with a different prefix ("Invalid Statement: " instead of "Statement wasn't well formed, can't be processed: ") and a different body.
Why: the message differs in both its prefix and its body, so a client keying on AWS's wording — or grouping errors by the "Statement wasn't well formed" prefix the way AWS's other PartiQL parse failures do — misclassifies this refusal entirely.
Fix: the clause-level validation of UPDATE in the ExecuteStatement handler's PartiQL validator, which must raise the statement-not-well-formed parse error rather than minting its own message, in the dynamodb service.
Traces: dynamodb.execute-statement-missing-clause-rejected.

Neither check is marked OWNED or ALREADY FILED in the replay report. The open issue 20261105T114920Z-dynamodb-6-net-new-partiql-and-updateexpression-causes.md and 20261103T065250Z-dynamodb-partiql-and-legacy-parameter-surface-10-root.md cover other PartiQL checks but not these two replay keys.

## Evidence
Cause 1 (dynamodb.execute-statement-malformed-literal-rejected):
Expected (AWS): ValidationException, Message == "Statement wasn't well formed, can't be processed: " for Statement="INSERT INTO \"<table>\" VALUE {'pk': 'py', 'n': 1e}".
Actual (emulator): AssertionError: unexpected message "Statement wasn't well formed, can't be processed: Unexpected term" (code was ValidationException, so only the message diverges).

Cause 2 (dynamodb.execute-statement-missing-clause-rejected):
Expected (AWS): ValidationException, Message == "Statement wasn't well formed, can't be processed: Expected data manipulation" for Statement="UPDATE \"<table>\" WHERE pk='seed'".
Actual (emulator): AssertionError: unexpected message 'Invalid Statement: an UPDATE must state at least one SET or REMOVE clause'.

In both cases the table name and key values in the statement are per-run values supplied by the request; the quoted messages above carry no request-derived part, so their text is wholly fixed.

## Steps to Reproduce
1. CreateTable(TableName=<t>, KeySchema=[{pk,HASH}], AttributeDefinitions=[{pk,S}], BillingMode=PAY_PER_REQUEST); wait for ACTIVE.
2. Cause 1: ExecuteStatement(Statement="INSERT INTO \"<t>\" VALUE {'pk': 'py', 'n': 1e}"). Observe ValidationException with Message "Statement wasn't well formed, can't be processed: Unexpected term"; AWS answers "Statement wasn't well formed, can't be processed: " (empty tail). Replay check dynamodb.execute-statement-malformed-literal-rejected.
3. Cause 2: ExecuteStatement(Statement="INSERT INTO \"<t>\" VALUE {'pk': 'seed', 'n': 1}") then ExecuteStatement(Statement="UPDATE \"<t>\" WHERE pk='seed'"). Observe ValidationException with Message 'Invalid Statement: an UPDATE must state at least one SET or REMOVE clause'; AWS answers "Statement wasn't well formed, can't be processed: Expected data manipulation". Replay check dynamodb.execute-statement-missing-clause-rejected.
