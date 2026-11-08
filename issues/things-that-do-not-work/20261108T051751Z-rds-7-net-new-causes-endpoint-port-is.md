Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.aurora-writer-reader-endpoint-query-routing rds.deep-rds-sql-truth-force-ssl-plaintext-login-rejected rds.create-shard-group-wrong-cluster-state-rejected rds.data-api-transaction-rollback-isolation rds.instance-automated-backup-lifecycle rds.read-replica-promote-lifecycle rds.snaprestore-final-snapshot-on-delete-restores-rows rds.rds-sql-connectivity-postgres-mysql
Issue-Key: rds.aurora-writer-reader-endpoint-query-routing|run#30|AssertionError|assert port == 5432, f"aurora-postgresql must listen on 5432, got {port!r}"
Issue-Key: rds.deep-rds-sql-truth-force-ssl-plaintext-login-rejected|run#21|AssertionError|assert port == _PORT, f"postgres must listen on {_PORT}; DescribeDBInstances says {port!r}"
Issue-Key: rds.create-shard-group-wrong-cluster-state-rejected|_error#18|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: rds.data-api-transaction-rollback-isolation|run#54|AssertionError|assert malformed_code == "ValidationException", (
Issue-Key: rds.instance-automated-backup-lifecycle|_poll_until#11|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: rds.read-replica-promote-lifecycle|run#17|KeyError|assert replica_before["StatusInfos"][0]["Status"] == "replicating", (
Issue-Key: rds.snaprestore-final-snapshot-on-delete-restores-rows|run#29|AssertionError|assert dst_addr != src_addr, (
Issue-Key: rds.rds-sql-connectivity-postgres-mysql|_sql_roundtrip_with_retry#12|Timeout|raise AssertionError(
Issue-Commit: 8b0f35809934c567f1bf7b94b8ce8845151fefb8

# rds: 7 net-new causes — Endpoint.Port is an ephemeral listener port on aurora-postgresql/postgres, CreateDBShardGroup skips the cluster-state gate, a malformed Data API transactionId is misclassified, retained automated backups are never created, a read replica reports no StatusInfos, a restored instance reuses the source endpoint, and the SQL-connectivity flow never completes

> _Note: this issue was identified against the code base at commit `8b0f35809934c567f1bf7b94b8ce8845151fefb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBInstances

## Description
This issue bundles 7 net-new root causes in the rds service, grounded in failing behavioural checks the suite report marked neither ALREADY FILED nor OWNED. Two of them are the same defect class as open issues whose replay keys do not cover these checks: 20261106T031005Z-rds-7-net-new-causes-endpoint-port-publishes.md (Cause 1, whose Issue-Trace-Ids do not include either check below) and 20261105T224225Z-rds-2-net-new-causes-every-db-instance.md (Cause 6, whose replay key for this check is the old poll Timeout, not today's endpoint-address assertion). A fixer must close all SEVEN causes below before submitting.

=== Cause 1 of 7: Endpoint.Port publishes an ephemeral internal listener port instead of the engine port ===
AWS: Smithy: Endpoint$Port — "Specifies the port that the database engine is listening on"; CreateDBInstanceMessage$Port defaults per engine family (RDS for PostgreSQL 5432, MySQL/MariaDB 3306, Oracle 1521, SQL Server 1433, Db2 50000) and is echoed when supplied. So DescribeDBInstances on a postgres or aurora-postgresql member created with Port omitted must report 5432.
Emulator: Endpoint.Port is a per-instance port drawn from the host's dynamic range — observed 42187 for an aurora-postgresql writer and 37493 for a postgres instance, a different value per instance. The requirement is the modelled engine default (or the requested Port); the emulator's own socket placement must not leak into the response.
Why: a client reading Endpoint.Port builds a connection string, security-group rule or proxy target that is not AWS's contract, and "postgres listens on 5432" — true of every real RDS instance — is false here.
Fix: the endpoint projection in the DescribeDBInstances/CreateDBInstance serializers and the instance state that records Port, decoupled from the local socket the data plane binds.
Traces: rds.aurora-writer-reader-endpoint-query-routing, rds.deep-rds-sql-truth-force-ssl-plaintext-login-rejected.

=== Cause 2 of 7: CreateDBShardGroup is accepted against a cluster that is not available ===
AWS: Smithy: CreateDBShardGroup models InvalidDBClusterStateFault (awsQueryError code InvalidDBClusterStateFault, HTTP 400) — "The requested operation can't be performed while the cluster is in this state." A CreateDBShardGroup issued while the Limitless cluster is stopped is refused with that fault and creates nothing; the identical call succeeds once the same cluster is started and available.
Emulator: the early call against the stopped cluster SUCCEEDS (the check's rejection helper reports "expected the call to be rejected, but it succeeded"), so the state gate is absent and a shard group is created against a cluster that cannot host it.
Why: a client gets a shard group whose backing cluster is stopped — AWS would have refused it — so an orchestration that relies on the refusal to sequence start-then-create silently proceeds, and the resulting resource state is one AWS can never produce.
Fix: the cluster-state precondition in the rds crate's CreateDBShardGroup handler — check the cluster's status before creating and refuse with InvalidDBClusterStateFault.
Traces: rds.create-shard-group-wrong-cluster-state-rejected.

=== Cause 3 of 7: a malformed Data API transactionId is misclassified as TransactionNotFoundException instead of being refused on format validation ===
AWS: recorded behavior only for the classification (rds-data's Smithy model lists TransactionNotFoundException at HTTP 404 for ExecuteStatement/CommitTransaction/RollbackTransaction, and BadRequestException at 400, but does not model the framework-level validation error). Recorded: a transactionId that is not a well-formed Data API transaction identifier is refused 400 ValidationException on input validation, BEFORE any transaction lookup; the modelled 404 TransactionNotFoundException is reserved for a WELL-FORMED id that names no live transaction, and its message names the transaction.
Emulator: a malformed transactionId is answered with code 'TransactionNotFoundException' and the message "INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction" — the wrong error class, and a message describing a terminated transaction rather than a rejected parameter.
Why: a client cannot tell a bad parameter (never retryable, fix the call) from a transaction that expired or was rolled back (re-begin and retry); it will re-begin a transaction in response to what is actually a malformed id, and a 404 where AWS returns 400 changes retry classification in the SDKs.
Fix: transactionId format validation in the rds-data ExecuteStatement/CommitTransaction/RollbackTransaction handlers, ordered ahead of the transaction lookup, with the lookup's TransactionNotFoundException left for well-formed unknown ids.
Traces: rds.data-api-transaction-rollback-isolation.

=== Cause 4 of 7: DeleteDBInstance with DeleteAutomatedBackups=false leaves no retained automated backup to describe ===
AWS: Smithy: DescribeDBInstanceAutomatedBackups takes DbiResourceId and returns DBInstanceAutomatedBackups (and refuses an unknown id with DBInstanceAutomatedBackupNotFound, HTTP 404). Deleting an instance that had BackupRetentionPeriod>0 with DeleteAutomatedBackups=false retains its automated backups: DescribeDBInstanceAutomatedBackups(DbiResourceId=<the deleted instance's DbiResourceId>) reports one entry whose DBInstanceIdentifier is the deleted instance's identifier, and DeleteDBInstanceAutomatedBackup then removes it.
Emulator: after the instance is deleted with DeleteAutomatedBackups=false, DescribeDBInstanceAutomatedBackups for that DbiResourceId never yields an entry — the probe polls for 120s and sees nothing (the check's probe swallows the client error, so the observed value is None). Note the scaffold-stub issue already filed for the unimplemented DescribeDBInstanceAutomatedBackups operation covers the operation existing at all; what is filed HERE is the retention behaviour the delete path must produce, which must still be closed once the read works.
Why: the documented backup-retention runbook (delete the instance but keep its backups, then restore or explicitly delete them) is unusable: the retained backup is invisible, so a client cannot verify retention, cannot restore from it, and cannot clean it up.
Fix: the automated-backup retention records in the rds crate's state — create the retained DBInstanceAutomatedBackup when DeleteDBInstance runs with DeleteAutomatedBackups=false and a non-zero BackupRetentionPeriod, and serve it from DescribeDBInstanceAutomatedBackups until DeleteDBInstanceAutomatedBackup removes it.
Traces: rds.instance-automated-backup-lifecycle.

=== Cause 5 of 7: a read replica reports no StatusInfos, so its replication state is unreadable ===
AWS: Smithy: DBInstance$StatusInfos — "The status of a read replica. If the DB instance isn't a read replica, the value is blank." For an instance created by CreateDBInstanceReadReplica, DescribeDBInstances returns a StatusInfos list with an entry whose StatusType is "read replication" and whose Status is "replicating" while replication is healthy.
Emulator: the member is absent entirely from the replica's DBInstance (the check raises KeyError: 'StatusInfos'), so a read replica is indistinguishable from a standalone instance on the one member that reports replication health.
Why: a client cannot tell whether a replica is replicating, has an error, or is in terminal replication failure — the documented way to check replica health returns nothing — and promotion/switchover orchestration keyed on "replicating" cannot proceed.
Fix: the DBInstance projection in the rds crate's DescribeDBInstances serializer and the replica state behind it — populate StatusInfos for read replicas (and leave it blank for non-replicas).
Traces: rds.read-replica-promote-lifecycle.

=== Cause 6 of 7: a snapshot-restored instance publishes the SAME Endpoint.Address as the source it was restored from ===
AWS: Smithy: DBInstance$Endpoint is an Endpoint whose Address is "the DNS address of the DB instance" — RDS mints one DNS name per instance derived from that instance's own DBInstanceIdentifier, so two instances never report the same Address, and an instance restored into a new identifier is reachable only at its own new address.
Emulator: the restored instance's Endpoint.Address equals the destroyed source's (both are the single shared listener address the emulator publishes for every instance), so the published endpoint does not identify an instance. The requirement is that the address be per-instance and derived from the instance identifier, not any particular host value, which is environment-specific.
Why: a client that connects to the restored instance's published endpoint cannot be sure which database it reaches, cannot verify a restore, and any CNAME, connection string or firewall rule keyed on the per-instance endpoint collapses onto one address.
Fix: the DB-instance endpoint publisher in the rds crate's state/data-plane layer — derive and store a per-instance address from the instance's own identifier and serve each instance behind it.
Traces: rds.snaprestore-final-snapshot-on-delete-restores-rows.

=== Cause 7 of 7: the postgres+mysql SQL-connectivity end-to-end flow never completes ===
AWS: recorded behavior only. The contract is Smithy's: CreateDBInstance drives an instance to DBInstanceStatus='available' in bounded time and publishes an Endpoint that accepts a native session for the engine, for both postgres and mysql; every state a client polls for is reached.
Emulator: the check is cut off at the harness cap ("Check timed out after 180.0s") with no assertion evaluated — the flow never reaches a state the poll accepts. Hypothesis: one of the two instances never settles into the status DescribeDBInstances must report, or its published endpoint never accepts a session, so the poll spends the whole budget. The check probes for its psycopg/pymysql drivers up front and raises the runner's own ImportError when they are missing, so a missing client is not the cause here.
Why: a client following the most basic provisioning path (create an instance, wait for available, connect) hangs instead of succeeding or being refused — the failure is invisible because nothing ever errors.
Fix: the DB-instance lifecycle in the rds crate's state (the creating->available transition DescribeDBInstances reports) and the engine endpoint it must bring up for postgres and mysql.
Traces: rds.rds-sql-connectivity-postgres-mysql.

## Evidence
Cause 1: AssertionError: aurora-postgresql must listen on 5432, got 42187; AssertionError: postgres must listen on 5432; DescribeDBInstances says 37493. Expected 5432 (engine default) in both.
Cause 2: AssertionError: expected the call to be rejected, but it succeeded: None — CreateDBShardGroup against a stopped Limitless cluster returned success; expected 400 InvalidDBClusterStateFault and no shard group.
Cause 3: AssertionError: a malformed transactionId must be refused on format validation with ValidationException, before the transaction lookup that produces TransactionNotFoundException; got 'TransactionNotFoundException': 'INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction'.
Cause 4: AssertionError: retained automated backup for <DbiResourceId> not met within 120s. Last result: None — DescribeDBInstanceAutomatedBackups never returned an entry after DeleteDBInstance(DeleteAutomatedBackups=False).
Cause 5: KeyError: 'StatusInfos' on the DBInstance returned by DescribeDBInstances for a read replica; expected a list with StatusType 'read replication' and Status 'replicating'.
Cause 6: AssertionError: the restored instance must have its own endpoint address, not the destroyed source's ('127.0.0.1') — the restored and source Endpoint.Address values are equal.
Cause 7: "Error: Check timed out after 180.0s" — no response body was compared at all.

## Steps to Reproduce
Cause 1: CreateDBCluster(Engine=aurora-postgresql) with a writer member, and separately CreateDBInstance(Engine=postgres) with Port omitted; poll to available and read Endpoint.Port — it must be 5432.
Cause 2: create a Limitless-capable aurora-postgresql cluster, StopDBCluster it, then CreateDBShardGroup(DBClusterIdentifier=<it>, DBShardGroupIdentifier=<new>, MaxACU=<n>) — must be refused InvalidDBClusterStateFault; then StartDBCluster, wait available, and repeat — must succeed.
Cause 3: on an Aurora Serverless v2 cluster with the Data API enabled, BeginTransaction, then RollbackTransaction/ExecuteStatement with transactionId set to a malformed value — read Error.Code/Error.Message.
Cause 4: CreateDBInstance(Engine=mysql, BackupRetentionPeriod=1), wait available, record DbiResourceId, DeleteDBInstance(SkipFinalSnapshot=True, DeleteAutomatedBackups=False), wait until the instance is gone, then DescribeDBInstanceAutomatedBackups(DbiResourceId=<recorded>) — expect one entry whose DBInstanceIdentifier is the deleted instance's; DeleteDBInstanceAutomatedBackup must then remove it, and an unknown DbiResourceId must be refused DBInstanceAutomatedBackupNotFound with "Automated backups could not be found for: <id>".
Cause 5: CreateDBInstance(Engine=oracle-ee) primary, CreateDBInstanceReadReplica from it, wait available, DescribeDBInstances on the replica — read StatusInfos.
Cause 6: CreateDBInstance A, write rows, DeleteDBInstance(A, FinalDBSnapshotIdentifier=S), wait for S, RestoreDBInstanceFromDBSnapshot(S -> B), wait available, and compare B's Endpoint.Address with A's recorded one — they must differ.
Cause 7: CreateDBInstance(Engine=postgres) and CreateDBInstance(Engine=mysql), poll each to available and open a native session to each published endpoint; the whole flow must complete well inside the 180s budget.
