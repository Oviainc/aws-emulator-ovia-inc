Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-execute-statement-malformed-literal-rejected
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-execute-statement-malformed-literal-rejected dynamodb-execute-statement-missing-clause-rejected dynamodb-execute-statement-quoting-rules dynamodb-frctraj-partiql-count-aggregate-and-unquoted-name dynamodb-partiql-from-clause-path-arity-rejected dynamodb-partiql-function-document-path-and-operand-count-rejected dynamodb-sfr-traj-partiql-select-quoting-mistakes dynamodb-ssbs-traj-partiql-sql-habits-for-status-tally dynamodb-trajwf-partiql-aggregate-and-distinct-unsupported
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|ExecuteStatement|4xx->4xx|$.message
Issue-Commit: dabd605678ea57cce68275edff8c08147ffd283a

# DynamoDB PartiQL malformed-statement refusal appends an emulator parser detail to AWS's fixed ValidationException text

> _Note: this issue was identified against the code base at commit `dabd605678ea57cce68275edff8c08147ffd283a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Cause 1 of 1: ExecuteStatement (and the PartiQL statements carried by BatchExecuteStatement/ExecuteTransaction) refuses a malformed statement with the right HTTP status and the right error code, but with a message that is AWS's fixed text plus an extra emulator-authored parser detail.

AWS: 400 ValidationException. Smithy: ExecuteStatement.Statement -> PartiQLStatement, required, @length min 1 max 8192; the operation's modeled client errors are ConditionalCheckFailedException / DuplicateItemException / ItemCollectionSizeLimitExceededException / ProvisionedThroughputExceededException / RequestLimitExceeded / ResourceNotFoundException / ThrottlingException / TransactionConflictException, so a statement that does not parse is reported through the service-common ValidationException (recorded behavior only for the message, since the model does not carry error prose). For this whole class of statement defects the recorded AWS response's message is the fixed text "Statement wasn't well formed, can't be processed: " with NOTHING after the colon-space: the trailing detail position is present but empty. The fixed text is everything up to and including "can't be processed: "; there is no variable part taken from the request for this class (AWS does not echo the offending token or a position here). The recorded 400 and the recorded __type both already match, so the error class and status are correct and only the message text diverges.

Emulator: returns the same 400 and the same error type, but the message is "Statement wasn't well formed, can't be processed: Unexpected term" — it substitutes its own parser's diagnostic ("Unexpected term") into the detail position that AWS leaves empty. The same substitution appears across the whole cluster of 20 recorded steps: malformed literals, a statement missing a required clause, quoting-rule violations, unquoted table/attribute names, COUNT/aggregate and DISTINCT statements, FROM-clause path arity defects and function/document-path operand-count defects all come back with the emulator's parser detail glued onto AWS's fixed prefix instead of AWS's bare prefix.

Why: a client that branches on the ValidationException message — a PartiQL console, an SDK retry/classification wrapper, or a test that asserts the refusal text — sees a message AWS never produces. Exact-match comparisons fail, and prefix/substring comparisons that look for the end of AWS's string fail too, so a caller cannot distinguish "statement did not parse" from the other PartiQL refusal families that legitimately do carry a detail after the colon. Because the emulator's detail is its own parser's vocabulary, it also leaks an implementation-specific token into a client-visible contract.

Note: the open issue 20261105T114920Z-dynamodb-6-net-new-partiql-and-updateexpression-causes.md records a DIFFERENT member of this message family (the unquoted-reserved-word case, where AWS's detail is the non-empty fixed text "Unexpected keyword") and does not cover this cluster: here the required detail is EMPTY, and the fix is to stop emitting a detail for this class rather than to emit a different one. The replay report leaves this cluster unowned.

Fix: the PartiQL statement parser/validator's error serializer on the ExecuteStatement / BatchExecuteStatement / ExecuteTransaction validation path — the code that formats the ValidationException message for a statement that fails to parse must emit AWS's fixed text with the detail position empty for this class, instead of interpolating the internal parser's diagnostic token.

Traces: fixture dynamodb-execute-statement-malformed-literal-rejected step task#6 is the representative; the cluster's other 19 recorded steps span dynamodb-execute-statement-malformed-literal-rejected (task#6, task#8, task#10), dynamodb-execute-statement-missing-clause-rejected (task#5, task#7, task#10), dynamodb-execute-statement-quoting-rules (task#6), dynamodb-frctraj-partiql-count-aggregate-and-unquoted-name (task#4, task#5), dynamodb-partiql-from-clause-path-arity-rejected (task#5, task#7), dynamodb-partiql-function-document-path-and-operand-count-rejected (task#5, task#6), dynamodb-sfr-traj-partiql-select-quoting-mistakes (task#6, task#7), dynamodb-ssbs-traj-partiql-sql-habits-for-status-tally (task#4, task#5, task#6) and dynamodb-trajwf-partiql-aggregate-and-distinct-unsupported (task#4, task#5).

## Evidence
Replay cluster divergence_key `service=dynamodb|ExecuteStatement|status=400->400|keys=$.message`, 20 affected steps, representative fixture `dynamodb-execute-statement-malformed-literal-rejected` step `task#6`.

Expected status 400, actual status 400 (status and `$.__type` match).
`$.message`: value_mismatch —
  expected: "Statement wasn't well formed, can't be processed: "
  actual:   "Statement wasn't well formed, can't be processed: Unexpected term"

Fixed text (required): `Statement wasn't well formed, can't be processed: `. Variable part for this error class: none — AWS leaves the detail after the colon-space empty and echoes no token, position or value from the request. The emulator's extra substring `Unexpected term` is the emulator's own parser diagnostic and is not part of the template.

## Steps to Reproduce
1. Create a DynamoDB table.
2. Call ExecuteStatement with a statement that does not parse — e.g. a SELECT carrying a malformed literal, a statement missing a required clause, an unquoted table name, or an aggregate/DISTINCT projection (any statement from the fixtures listed under Traces).
3. Observe the 400 ValidationException body: `$.message` is `Statement wasn't well formed, can't be processed: Unexpected term` where AWS returns `Statement wasn't well formed, can't be processed: ` with an empty detail.
4. Re-replay fixture `dynamodb-execute-statement-malformed-literal-rejected` step `task#6` (and the other steps listed under Traces) to confirm the fix.
