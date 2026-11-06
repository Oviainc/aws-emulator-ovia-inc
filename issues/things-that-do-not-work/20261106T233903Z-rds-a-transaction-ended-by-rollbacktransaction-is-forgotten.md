Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.data-api-transaction-rollback-isolation
Issue-Key: rds.data-api-transaction-rollback-isolation|run#49|AssertionError|assert stale_message == (
Issue-Commit: 45af2140f358870cdf193287cbc1a2501641d010

# rds: a transaction ended by RollbackTransaction is forgotten, so a later statement on that id is refused with the generic "The transaction ID wasn't found" instead of AWS's terminated-transaction message

> _Note: this issue was identified against the code base at commit `45af2140f358870cdf193287cbc1a2501641d010`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
One cause, in the rds-data transaction state the RDS Data API serves for a cluster this check creates through rds.

Cause 1 of 1: a well-formed transactionId whose transaction was ended by RollbackTransaction is reported as an id the service has never seen.
AWS: Smithy (rdsdata RdsDataService): TransactionNotFoundException is errorKind client @httpError(404) with the shape doc "The transaction ID wasn't found"; ExecuteStatement also models ValidationException-class framework validation for a malformed id. For a well-formed id whose transaction has already reached a terminal state, AWS answers HTTP 404 TransactionNotFoundException with the message "INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction" — that whole sentence is fixed text, with no variable part. A syntactically invalid id is refused earlier, with ValidationException and the message "Invalid transaction ID".
Emulator: HTTP 404, code TransactionNotFoundException (correct), message "The transaction ID wasn't found" — the generic not-found text, i.e. the rolled-back transaction was deleted from state and cannot be distinguished from an id that never existed.
Why: the two conditions a client must separate collapse. A client that retries or re-begins on "not found" but must stop and fix its statement on "your transaction was terminated" takes the wrong branch, and operator diagnostics report a vanished id where AWS reports a transaction the client's own rollback killed.
Fix: the rds-data transaction state machine and the refusal built from it — a committed or rolled-back transaction must be retained in a terminal state and reported with the termination message by ExecuteStatement/CommitTransaction/RollbackTransaction, instead of being removed from the store.
Traces: rds.data-api-transaction-rollback-isolation (the same defect is filed for the rds-data group's own checks in this sweep).

Related: the open wire issue 20261023T045849Z-rds-data-skips-resource-secret-and-transaction-preconditions.md records this message for CommitTransaction/RollbackTransaction after a FAILED statement (its causes 8 and 9); it carries no behavioral check and does not cover the rolled-back-transaction path asserted here.

## Evidence
Check rds.data-api-transaction-rollback-isolation, after RollbackTransaction succeeds on the transaction:

AssertionError: unexpected TransactionNotFoundException message for the rolled-back id: "The transaction ID wasn't found"

Expected: ExecuteStatement with the rolled-back transactionId -> HTTP 404, Code TransactionNotFoundException, Message "INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction".
Actual: HTTP 404, Code TransactionNotFoundException, Message "The transaction ID wasn't found".
The check reaches this assertion only after the rollback itself behaved correctly (status 'Rollback complete', both rolled-back rows gone), so only the terminal-state refusal message diverges. The cluster id, transaction id and table name in the run are per-run values and are not requirements.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. rds: create an Aurora Serverless v2 cluster with EnableHttpEndpoint=true and ManageMasterUserPassword=true, plus a db.serverless writer; wait until the Data API answers.
3. rds-data: BeginTransaction, run an INSERT inside it, then RollbackTransaction (observe transactionStatus 'Rollback complete').
4. rds-data: ExecuteStatement again with that same transactionId.
5. Expect 404 TransactionNotFoundException with "INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction"; observe 404 with "The transaction ID wasn't found".
