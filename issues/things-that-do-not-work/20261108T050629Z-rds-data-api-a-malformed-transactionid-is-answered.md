Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.data-api-transaction-rollback-isolation
Issue-Key: rds.data-api-transaction-rollback-isolation|run#54|AssertionError|assert malformed_code == "ValidationException", (
Issue-Commit: 8b0f35809934c567f1bf7b94b8ce8845151fefb8

# rds Data API: a malformed transactionId is answered as a terminated transaction (404 TransactionNotFoundException) instead of 400 ValidationException "Invalid transaction ID"

> _Note: this issue was identified against the code base at commit `8b0f35809934c567f1bf7b94b8ce8845151fefb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
One cause, in the rds-data ExecuteStatement precondition chain reached from an rds-provisioned Aurora Serverless v2 cluster (the suite lists this check under the `rds` service).

Cause 1 of 1: a transactionId that cannot be a Data API transaction id at all is not refused on format; it falls through to the transaction store and is reported with the terminated-transaction error.
AWS: Smithy (com.amazonaws.rdsdata#ExecuteStatement): `transactionId` targets `Id` with @length max 192; the operation models TransactionNotFoundException as errorKind client @httpError(404) (doc "The transaction ID wasn't found"). Recorded behavior: a syntactically invalid transactionId is refused BEFORE any transaction lookup, with ValidationException at HTTP 400 and the message "Invalid transaction ID" — entirely fixed text, no variable part. A WELL-FORMED id whose transaction was rolled back/terminated is a different outcome: 404 TransactionNotFoundException with the fixed text "INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction" (that leg of the same check passes today). The two refusals must not collapse onto one code.
Emulator: ExecuteStatement with transactionId = an arbitrary non-id string answers TransactionNotFoundException with "INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction" — i.e. the same code and message it (correctly) returns for the stale, well-formed id, so format validation never runs.
Why: the error class is the client's branch point. A caller that passed a corrupted/truncated id is told its transaction was terminated by a previous call, so it concludes work was lost and re-begins, instead of being told its request is malformed; and it cannot distinguish "your id is not an id" from "your transaction ended".
Fix: the transactionId validation leg of the ExecuteStatement handler's precondition chain — validate the id's format and refuse with ValidationException (400, "Invalid transaction ID") ahead of the transaction-store lookup that yields TransactionNotFoundException. Related open issues, neither of which carries this check: 20261107T062403Z-rds-data-a-terminated-transaction-is-forgotten-so.md (the terminal-state side of the same chain) and 20261023T045849Z-rds-data-skips-resource-secret-and-transaction-preconditions.md (wire corpus, other inputs).
Traces: rds.data-api-transaction-rollback-isolation.

## Evidence
Cause 1 — check rds.data-api-transaction-rollback-isolation:
AssertionError: a malformed transactionId must be refused on format validation with ValidationException, before the transaction lookup that produces TransactionNotFoundException; got 'TransactionNotFoundException': 'INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction'
Expected: Code ValidationException, HTTP 400, Message "Invalid transaction ID" (fixed text), and a code DIFFERENT from the one the stale well-formed id draws.
Actual: Code TransactionNotFoundException, HTTP 404, Message "INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction" — identical to the stale-id refusal that the preceding assertions of the same check already accepted.

## Steps to Reproduce
1. Build and boot the emulator from this checkout; replay the behavioral check rds.data-api-transaction-rollback-isolation.
2. By hand: rds CreateDBCluster (Engine=aurora-postgresql, ManageMasterUserPassword=true, EnableHttpEndpoint=true, DatabaseName set) plus a db.serverless CreateDBInstance; wait for 'available'.
3. rds-data BeginTransaction on that cluster, then RollbackTransaction, then POST /Execute with that (well-formed) transactionId -> 404 TransactionNotFoundException "INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction" (correct today).
4. POST /Execute `SELECT 1` on the same cluster with transactionId="sim2real-behav-no-such-transaction-id" -> expect 400 ValidationException "Invalid transaction ID"; observe the same 404 TransactionNotFoundException / terminated-transaction message as step 3.
