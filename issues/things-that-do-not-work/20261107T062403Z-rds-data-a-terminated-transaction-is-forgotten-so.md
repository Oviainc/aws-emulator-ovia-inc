Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds-data.rds-data-rollback-transaction-on-terminal-transaction-rejected-and-state-kept
Issue-Shape-Id: com.amazonaws.rdsdata#RdsDataService
Issue-Key: rds-data.rds-data-rollback-transaction-on-terminal-transaction-rejected-and-state-kept|_poll#4|Timeout|time.sleep(interval_s)
Issue-Commit: 547f35aa751bdfccaa69888f930e73773cf7bea8

# rds-data: a terminated transaction is forgotten, so RollbackTransaction/ExecuteStatement on a committed or rolled-back transactionId answers the generic "The transaction ID wasn't found", and the check never reaches a verdict inside its budget

> _Note: this issue was identified against the code base at commit `547f35aa751bdfccaa69888f930e73773cf7bea8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RollbackTransaction

## Description
Two causes, both exercised by the single failing check rds-data.rds-data-rollback-transaction-on-terminal-transaction-rejected-and-state-kept (the only failure in this replay that is neither ALREADY FILED nor OWNED).

Cause 1 of 2: a transaction that ENDED (committed, or rolled back) is deleted from the cluster's transaction map, so a later RollbackTransaction or ExecuteStatement carrying that id is answered as an id the service never knew, with the not-found sentence instead of AWS's terminated-transaction sentence.
AWS: Smithy rdsdataservice models RollbackTransaction$transactionId (Id, @length max 192) and TransactionNotFoundException @httpError(404) ("The transaction ID wasn't found"). For an id the service DID issue and that is now terminal, AWS still answers HTTP 404 TransactionNotFoundException but with the fixed message "INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction" — that sentence is entirely fixed text, it carries no value from the request or from state, and it is the message for the terminal-transaction class on RollbackTransaction, CommitTransaction and ExecuteStatement alike. Refusing the call must change nothing: the committed row stays readable and the rolled-back write stays absent. CommitTransaction on a live id answers transactionStatus "Transaction Committed"; the first RollbackTransaction answers "Rollback complete".
Emulator: the handler removes the transaction on commit and on rollback (services/service-com-amazonaws-rdsdata-rdsdataservice/src/ops/rollback_transaction.rs: `cluster.transactions.remove(&input.transaction_id).ok_or(SqlError::TransactionNotFound)?`), so a second call on that id is mapped to TransactionNotFoundException carrying the generic lookup-miss message "The transaction ID wasn't found". The same message for the same class is visible in this replay on the sibling rds-data check commit-transaction-twice-rejected, whose output is `AssertionError: unexpected refusal message: "The transaction ID wasn't found"`; the state is service-wide, so the RollbackTransaction and ExecuteStatement arms this check grades carry it too.
Why: the message is how a client distinguishes "this id is garbage/belongs elsewhere" from "your transaction was already ended by a previous call". A client that retries or re-begins on the generic sentence re-does work AWS told it was already committed, and tooling that matches AWS's INVALID_ARGUMENT text sees no match at all.
Fix: the transaction store in the rds-data service state (a terminal transaction must be REMEMBERED as terminated rather than deleted) plus the SqlError-to-modelled-error mapping shared by rollback_transaction.rs, commit_transaction.rs and execute_statement.rs, so a terminal id yields the terminated sentence and an unknown id keeps the not-found sentence.
Traces: rds-data.rds-data-rollback-transaction-on-terminal-transaction-rejected-and-state-kept (grades RollbackTransaction on a committed id, a second RollbackTransaction on a rolled-back id, and ExecuteStatement on a rolled-back id, and asserts the table still holds only the committed row after each refusal). Related but NOT covering it: 20261106T233903Z-rds-data-8-net-new-root-causes-executestatement.md carries commit-transaction-twice-rejected, and 20261106T233903Z-rds-a-transaction-ended-by-rollbacktransaction-is-forgotten.md is filed under service rds; neither carries this check's replay key.

Cause 2 of 2: this check reaches no verdict at all — it is cut off at the harness cap with `Error: Check timed out after 60.0s`, so not one of its assertions is evaluated.
AWS: recorded behavior only. The check is a pure dp:api check (it opens no native engine session): it provisions an aurora-postgresql Serverless v2 cluster with ManageMasterUserPassword=true and a db.serverless member, polls DescribeDBInstances/DescribeDBClusters to 'available' and MasterUserSecret.SecretStatus to 'active', probes the Data API with `SELECT 1`, grades, and then deletes member and cluster INLINE, polling each to removal. On AWS the whole shape completes; against an emulator every one of those settles in milliseconds of work, so the only thing that can consume a 60 s budget is a state the emulator never reaches, which makes a poll loop spin to its own ceiling (member 1200 s at 15 s, cluster 600 s at 15 s, secret 300 s at 20 s, Data-API warm 300 s at 2 s, each teardown 900 s at 20 s).
Emulator: no response is recorded for the step it hangs on — the only output is the timeout. Hypothesis: the emulator's RDS lifecycle is wall-clock based (services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs: PendingStatus::in_a_moment / removal_in_a_moment settle a record only once STATUS_TRANSITION_DELAY has elapsed), so each of this check's five settle points costs a full poll interval and the inline DeleteDBInstance/DeleteDBCluster-to-removal waits are paid inside the same budget; a member or cluster whose Removal never settles (or a MasterUserSecret whose SecretStatus never becomes 'active') leaves the loop spinning. Sibling rds-data checks that share the provisioning helper DO reach their grading assertions in this replay, so provisioning alone is not the whole cost — this check is the one that also tears down inline.
Why: the behaviour under test (that a terminal transactionId is refused and that neither refusal changes table contents) is unobservable: the operation answers nothing a client can grade, and a client scripting provision → use → delete sees a lifecycle that never finishes.
Fix: the rds service's DB-instance/DB-cluster lifecycle settling and delete-to-removal path (state.rs settle_due_transitions and the Delete handlers), and the rds-managed MasterUserSecret projection they publish — every transition a caller polls for must become due promptly rather than only after a real-time delay.
Traces: rds-data.rds-data-rollback-transaction-on-terminal-transaction-rejected-and-state-kept.

## Evidence
Failing check (rds-data, 11/34 passed in this replay):

#### [mismatch] rds-data.rds-data-rollback-transaction-on-terminal-transaction-rejected-and-state-kept
Error: Check timed out after 60.0s

Cause 2 — Expected: the check runs to a verdict (provision, grade, inline teardown). Actual: no response for the step it hangs on; the harness cuts it at 60 s with no assertion evaluated.

Cause 1 — what the check grades, and what the emulator answers for the same class elsewhere in this same service in this same replay:
Expected (all three terminal-id refusals): HTTP 404, Code TransactionNotFoundException, Message "INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction" (fixed text, no variable part), with the table still holding exactly the committed row.
Actual (rds-data.commit-transaction-twice-rejected, same terminal-transaction class):
AssertionError: unexpected refusal message: "The transaction ID wasn't found"
Code path: services/service-com-amazonaws-rdsdata-rdsdataservice/src/ops/rollback_transaction.rs lines 203-206 and 232-239 — the transaction is `remove`d on rollback and a later lookup miss is reported as TransactionNotFoundException with SqlError::TransactionNotFound's generic message; commit takes the same path, so no terminal id is distinguishable from an id that was never issued.
Also expected on the happy path of this check: CommitTransaction transactionStatus "Transaction Committed", the first RollbackTransaction transactionStatus "Rollback complete".

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. rds: CreateDBSubnetGroup over the run's subnets, then CreateDBCluster (Engine=aurora-postgresql, DatabaseName set, MasterUsername set, ManageMasterUserPassword=true, EnableHttpEndpoint=true, ServerlessV2ScalingConfiguration MinCapacity 0.5 / MaxCapacity 4.0) and CreateDBInstance (DBInstanceClass=db.serverless) in that cluster.
3. Cause 2: poll DescribeDBInstances / DescribeDBClusters for 'available' and DescribeDBClusters for MasterUserSecret.SecretStatus 'active', then DeleteDBInstance and DeleteDBCluster (SkipFinalSnapshot=true) and poll each until it is gone. Expect every transition to become due promptly; observe the check budget (60 s) exhausted before a verdict.
4. Cause 1: rds-data BeginTransaction; ExecuteStatement an INSERT on that transactionId; CommitTransaction (expect transactionStatus "Transaction Committed"); then RollbackTransaction with the SAME transactionId. Expect HTTP 404 TransactionNotFoundException with message "INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction"; observe "The transaction ID wasn't found".
5. Repeat with a second transaction that is rolled back first: a second RollbackTransaction on it, and an ExecuteStatement carrying it, must both be refused with the same 404/code/message, and a later SELECT must still show only the committed row.
6. Replay rds-data.rds-data-rollback-transaction-on-terminal-transaction-rejected-and-state-kept.
