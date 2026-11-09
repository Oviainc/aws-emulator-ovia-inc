Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds-data.transaction-expires-after-idle-timeout
Issue-Shape-Id: com.amazonaws.rdsdata#RdsDataService
Issue-Key: rds-data.transaction-expires-after-idle-timeout|run#11|AssertionError|assert error.get("Message") == "INVALID_ARGUMENT: Transaction error: transaction has been terminated due to an idleness", f"unexpected refusal message: {error.g
Issue-Commit: ab58585e96332c13ed2b8d30023e29e5d03b1c39

# rds-data: an idle-expired transaction is refused with the generic terminated-transaction sentence instead of AWS's idleness sentence

> _Note: this issue was identified against the code base at commit `ab58585e96332c13ed2b8d30023e29e5d03b1c39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CommitTransaction

## Description
One net-new root cause.

Cause 1 of 1: a transaction that the service itself terminated because it sat idle past the documented 3-minute inactivity window is refused with the message AWS reserves for a transaction a PREVIOUS CALL ended, so the idleness class is indistinguishable from the commit/rollback class.

AWS:      Smithy rdsdataservice models TransactionNotFoundException @error("client") @httpError(404) ("The transaction ID wasn't found") on CommitTransaction and ExecuteStatement alike, and BeginTransaction documents that "a transaction is terminated and rolled back automatically after 3 minutes of inactivity". For an id the service issued and then terminated for IDLENESS, AWS answers HTTP 404 / TransactionNotFoundException with the fixed sentence "INVALID_ARGUMENT: Transaction error: transaction has been terminated due to an idleness". That sentence is entirely fixed text — it carries no value from the request or from state. It is distinct from the sentence for an id a previous call ended, "INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction", which AWS uses only when the client's own CommitTransaction/RollbackTransaction/failed-ExecuteStatement ended it. Both CommitTransaction and ExecuteStatement on the idle-expired id answer the idleness sentence; the positive controls on a live id still answer transactionStatus "Transaction Committed".
Emulator: after the idle window elapses, CommitTransaction (and, by the same mapping, ExecuteStatement) on that transactionId answers the terminated-by-previous-call sentence: 'INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction'. The error code (TransactionNotFoundException) matches; only the message diverges, so the emulator collapses two distinct refusal classes into one.
Why:      the message is the only signal that distinguishes "your client ended this transaction" from "the service reclaimed it because you left it idle". A client that matches AWS's idleness text to decide it must re-BeginTransaction and replay the work sees no match, and a client that reads the previous-call sentence concludes its own code already committed or rolled back — so work the service silently rolled back is never replayed.
Fix:      the rds-data transaction store in the service state must record WHY a transaction became terminal (idle-timeout reclamation vs. client-issued commit/rollback vs. a failed statement), and the SqlError-to-modelled-error mapping shared by commit_transaction.rs, rollback_transaction.rs and execute_statement.rs must select the idleness sentence for the reclaimed case. Open issue 20261107T062403Z-rds-data-a-terminated-transaction-is-forgotten-so.md describes the sibling previous-call sentence for commit/rollback-terminated ids and does not carry this check's replay key; it covers only that class, not the idleness class this issue reports.
Traces:   rds-data.transaction-expires-after-idle-timeout (BeginTransaction, idle 260 s with no call referencing the id, then CommitTransaction and ExecuteStatement on it).

## Evidence
Check rds-data.transaction-expires-after-idle-timeout [mismatch]:

AssertionError: unexpected refusal message: 'INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction'

Expected (CommitTransaction on the idle-expired transactionId): HTTP 404, Code TransactionNotFoundException, Message "INVALID_ARGUMENT: Transaction error: transaction has been terminated due to an idleness" — wholly fixed text, no request- or state-derived part.
Actual: HTTP 404, Code TransactionNotFoundException (matches), Message "INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction" — the fixed sentence for a DIFFERENT class.

The check asserts the same code and message for the independent ExecuteStatement arm on the same dead id, which the same mapping answers.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Provision an Aurora Serverless v2 cluster with the Data API enabled and its admin credential in Secrets Manager.
3. Positive control: rds-data BeginTransaction, then CommitTransaction on it — expect transactionStatus "Transaction Committed".
4. rds-data BeginTransaction again. Make NO further call that references the returned transactionId and wait past the documented 3-minute inactivity window (the check waits 260 s).
5. CommitTransaction with that transactionId. Expect HTTP 404 / TransactionNotFoundException / "INVALID_ARGUMENT: Transaction error: transaction has been terminated due to an idleness"; observe "INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction".
6. ExecuteStatement (sql="SELECT 1") with the same transactionId. Expect the same 404/code/message; observe the same divergent sentence.
7. Replay rds-data.transaction-expires-after-idle-timeout.
