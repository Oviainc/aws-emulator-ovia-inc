Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__rds-data__secretsmanager.concurrent-data-api-and-native-increments-lose-no-update rds__rds-data__secretsmanager.data-api-and-native-mysql-sessions-read-each-others-writes rds__rds-data__secretsmanager.data-api-insert-is-read-by-a-native-postgres-session rds__rds-data__secretsmanager.data-api-open-transaction-is-invisible-to-a-native-session-until-commit rds__rds-data__secretsmanager.data-api-row-lock-makes-a-native-nowait-lock-rejected-until-commit rds__rds-data__secretsmanager.mysql-lock-wait-timeout-rejects-an-update-of-a-row-a-data-api-transaction-holds rds__rds-data__secretsmanager.mysql-repeatable-read-snapshot-ignores-a-later-commit-until-it-ends rds__rds-data__secretsmanager.native-insert-is-read-by-the-data-api-postgres rds__rds-data__secretsmanager.native-uncommitted-write-is-invisible-to-a-second-session-under-read-committed rds__rds-data__secretsmanager.typed-values-agree-between-the-data-api-and-a-native-session rds__rds-data__secretsmanager.execute-statement-with-another-clusters-transaction-id-rejected
Issue-Key: rds__rds-data__secretsmanager.concurrent-data-api-and-native-increments-lose-no-update|_connect#1|Timeout|time.sleep(2)
Issue-Key: rds__rds-data__secretsmanager.data-api-and-native-mysql-sessions-read-each-others-writes|_connect#1|Timeout|time.sleep(2)
Issue-Key: rds__rds-data__secretsmanager.data-api-insert-is-read-by-a-native-postgres-session|_connect#1|Timeout|time.sleep(2)
Issue-Key: rds__rds-data__secretsmanager.data-api-open-transaction-is-invisible-to-a-native-session-until-commit|_connect#1|Timeout|time.sleep(2)
Issue-Key: rds__rds-data__secretsmanager.data-api-row-lock-makes-a-native-nowait-lock-rejected-until-commit|_connect#1|Timeout|time.sleep(2)
Issue-Key: rds__rds-data__secretsmanager.mysql-lock-wait-timeout-rejects-an-update-of-a-row-a-data-api-transaction-holds|_connect#1|Timeout|time.sleep(2)
Issue-Key: rds__rds-data__secretsmanager.mysql-repeatable-read-snapshot-ignores-a-later-commit-until-it-ends|_connect#1|Timeout|time.sleep(2)
Issue-Key: rds__rds-data__secretsmanager.native-insert-is-read-by-the-data-api-postgres|_connect#1|Timeout|time.sleep(2)
Issue-Key: rds__rds-data__secretsmanager.native-uncommitted-write-is-invisible-to-a-second-session-under-read-committed|_connect#1|Timeout|time.sleep(2)
Issue-Key: rds__rds-data__secretsmanager.typed-values-agree-between-the-data-api-and-a-native-session|_connect#1|Timeout|time.sleep(2)
Issue-Key: rds__rds-data__secretsmanager.execute-statement-with-another-clusters-transaction-id-rejected|_grade#2|AssertionError|assert code == "ValidationException", (
Issue-Commit: 45af2140f358870cdf193287cbc1a2501641d010

# rds__rds-data__secretsmanager: 2 net-new root causes — the cluster's published writer endpoint serves no engine wire protocol, so every Data-API/native-session check hangs to its budget, and another cluster's transactionId is refused with the wrong error class

> _Note: this issue was identified against the code base at commit `45af2140f358870cdf193287cbc1a2501641d010`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Two net-new causes. Clusters now provision in this group (the one check that needs no native session reaches its grading assertions, so CreateDBCluster with ManageMasterUserPassword, the managed master secret, DescribeDBInstances/DescribeDBClusters reaching 'available' and the Data API all answer) — what is left is the native surface and one refusal class.

Cause 1 of 2: nothing listens with the database engine's own wire protocol on the endpoint the cluster publishes, so a native session can never be opened.
AWS: recorded behavior only (Smithy AmazonRDSv19: DBCluster$Endpoint and DBCluster$Port are the cluster's writer address; DBCluster$MasterUserSecret carries the RDS-managed credential). An Aurora cluster whose writer is 'available' accepts a PostgreSQL (5432) or MySQL (3306) connection on exactly that Endpoint/Port, as the master user with the password held in the managed secret, into the cluster's DatabaseName — the connection completes within seconds of the writer becoming available, and `SELECT current_database(), current_user` answers for the same database and login the Data API uses.
Emulator: the connection attempt never succeeds. Each check retries `connect(host=<the Endpoint the emulator published>, port=<the Port it published>, user=<master user>, password=<the managed secret's password>, dbname=<DatabaseName>, connect_timeout=20)` every 2 s within a 600 s budget and is still failing when the check's 60 s budget expires, so all ten checks end `Error: Check timed out after 60.0s` with no verdict. The driver import is re-raised immediately by the check, so this is not a missing client library: it is the announced address not answering the engine protocol.
Why: the emulator advertises an Endpoint/Port that is not reachable with the protocol an RDS endpoint exists to speak, so the whole cross-surface contract is unobservable: that the Data API and a native session are two views of ONE database (writes visible both ways, the created table in the native catalog, the same database and user), and the engine semantics that only the native side can show (an open Data API transaction invisible until commit, a Data-API-held row lock making a NOWAIT lock fail and a plain UPDATE hit the lock-wait timeout, REPEATABLE READ snapshots, READ COMMITTED visibility, concurrent increments losing no update, typed values agreeing across both surfaces). A client that uses the Data API for some paths and a driver for others cannot run at all.
Fix: the rds service's cluster/instance provisioning must stand up a listener for the engine's wire protocol at the Endpoint/Port it reports (authenticating the master user against the managed secret and serving the same store the rds-data ExecuteStatement path uses), or else the endpoint it publishes must be the address that is actually served. This is one cause: every one of the ten checks fails at this single step, before any assertion of theirs runs.
Traces: concurrent-data-api-and-native-increments-lose-no-update, data-api-and-native-mysql-sessions-read-each-others-writes, data-api-insert-is-read-by-a-native-postgres-session, data-api-open-transaction-is-invisible-to-a-native-session-until-commit, data-api-row-lock-makes-a-native-nowait-lock-rejected-until-commit, mysql-lock-wait-timeout-rejects-an-update-of-a-row-a-data-api-transaction-holds, mysql-repeatable-read-snapshot-ignores-a-later-commit-until-it-ends, native-insert-is-read-by-the-data-api-postgres, native-uncommitted-write-is-invisible-to-a-second-session-under-read-committed, typed-values-agree-between-the-data-api-and-a-native-session.

Cause 2 of 2: ExecuteStatement on cluster B carrying a transactionId cluster A issued is refused as an unknown transaction instead of an invalid one.
AWS: Smithy: ExecuteStatement$transactionId targets Id (@length max 192); TransactionNotFoundException is @httpError(404) for an id this resource has (had) and no longer resolves, while an id that cannot belong to the named resource is refused by framework validation. AWS answers ValidationException with the message "Invalid transaction ID" (fixed text, no variable part) and writes nothing to cluster B; the transaction stays usable on cluster A, which then inserts and commits.
Emulator: answers TransactionNotFoundException with "The transaction ID wasn't found" — it resolves transaction ids in a resource-independent store and reports a lookup miss rather than validating the id against the resourceArn.
Why: the error class is the client's branch point. A client told 'not found' re-begins the transaction (losing work it believes is lost) where AWS tells it the id does not belong to this cluster and the transaction is still alive on the other one.
Fix: the transactionId validation in the ExecuteStatement handler's precondition chain — bind each transaction to the resourceArn that opened it and refuse a foreign id at validation, before the transaction-store lookup.
Traces: execute-statement-with-another-clusters-transaction-id-rejected.

Related: the open issue 20261104T044754Z-rds-rds-data-secretsmanager-createdbcluster-refuses-managemasteruserpassword-with.md lists these eleven checks against the earlier 501 refusal of CreateDBCluster$ManageMasterUserPassword; that refusal no longer occurs in this replay (checks now get past provisioning), so neither cause here is covered by it. 20261023T045849Z-rds-data-skips-resource-secret-and-transaction-preconditions.md covers wire-corpus transactionId validation for other inputs and carries no behavioral check.

## Evidence
Cause 1 — all ten dp:native checks in the group end with:
Error: Check timed out after 60.0s
The shared helper they hang in: `_await_cluster` completes the RDS waits and the Data API probe, then calls `_connect(db, budget_s=600)`, which loops `psycopg.connect(...)` / `pymysql.connect(...)` on `(cluster["Endpoint"], cluster["Port"])` every 2 s and re-raises ImportError immediately (so a missing driver would surface at once, not as a timeout). The single check in this group tagged dp:api rather than dp:native — execute-statement-with-another-clusters-transaction-id-rejected, whose provisioning helper never calls _connect — instead reaches its grading assertions, which is what localises the hang to the native connect.
Expected: a native session opens on the published writer endpoint within seconds of the writer reporting 'available', and `SELECT current_database(), current_user` returns the cluster's DatabaseName and master user.
Actual: no connection within the check budget; no assertion of any of the ten checks is ever evaluated.

Cause 2 — check execute-statement-with-another-clusters-transaction-id-rejected:
AssertionError: ExecuteStatement on cluster B with cluster A's transactionId must be refused; got 'TransactionNotFoundException': "The transaction ID wasn't found"
Expected: Code ValidationException, Message "Invalid transaction ID" (and cluster B unchanged).
Actual: Code TransactionNotFoundException, Message "The transaction ID wasn't found".

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite group rds__rds-data__secretsmanager.
2. Cause 1 by hand: rds CreateDBSubnetGroup, then CreateDBCluster (Engine=aurora-postgresql or aurora-mysql, DatabaseName set, MasterUsername set, ManageMasterUserPassword=true, EnableHttpEndpoint=true) plus a db.serverless CreateDBInstance; wait for 'available' and for MasterUserSecret.SecretStatus 'active'; read the password with secretsmanager GetSecretValue; then open a PostgreSQL/MySQL connection to DBCluster.Endpoint:DBCluster.Port as that user into DatabaseName. Expect the session to open and `SELECT current_database(), current_user` to answer; observe the connection never succeeding.
3. Cause 2 by hand: provision two such clusters A and B; rds-data BeginTransaction on A; ExecuteStatement on B with that transactionId. Expect 400 ValidationException "Invalid transaction ID"; observe 404 TransactionNotFoundException "The transaction ID wasn't found".
