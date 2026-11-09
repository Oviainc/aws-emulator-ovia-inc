Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__rds-data__secretsmanager.aurora-postgresql-failover-preserves-data-api-writes-on-both-surfaces rds__rds-data__secretsmanager.data-api-rollback-and-a-native-rollback-leave-no-trace-on-aurora-postgresql rds__rds-data__secretsmanager.data-api-rows-survive-cluster-snapshot-restore
Issue-Key: rds__rds-data__secretsmanager.aurora-postgresql-failover-preserves-data-api-writes-on-both-surfaces|_native_rows#49|UndefinedTable|cur.execute(statement)
Issue-Key: rds__rds-data__secretsmanager.data-api-rollback-and-a-native-rollback-leave-no-trace-on-aurora-postgresql|_rows#2|UndefinedTable|cur.execute(statement, args)
Issue-Key: rds__rds-data__secretsmanager|RestoreDBClusterFromSnapshot|NotImplemented
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# rds__rds-data__secretsmanager: Data API writes to an Aurora PostgreSQL cluster are invisible to a native session (the table does not exist), and RestoreDBClusterFromSnapshot is NotImplemented

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
2 net-new root causes where the Data API and the native PostgreSQL endpoint of one Aurora cluster do not share a database.

Cause 1 of 2: a table created and written through the Data API does not exist for a native psql session on the same cluster.
AWS: the Data API and a native connection are two front doors onto the same Aurora database. A CREATE TABLE + INSERT executed with ExecuteStatement is immediately visible to a native session connected to the same cluster and database (subject only to transaction visibility), and it survives a failover. Recorded behavior only for the sharing itself; it is the premise of every mixed-surface flow.
Emulator: after the Data API creates and populates the table, the native session's query fails with UndefinedTable 'no such table: <table>' - the two surfaces are backed by different stores. The same divergence shows up on a rollback flow, where the native session cannot see the table the Data API created either.
Why: this is a silent data-loss class. A client that writes through the Data API and reads with a driver (or vice versa) sees an empty, non-existent schema and no error pointing at the cause; failover-durability and rollback-visibility contracts cannot hold at all.
Fix: the storage binding behind the rds-data handlers and the native SQL front-end - both must resolve to the same database for the same resourceArn/database pair.
Traces: rds__rds-data__secretsmanager.aurora-postgresql-failover-preserves-data-api-writes-on-both-surfaces, rds__rds-data__secretsmanager.data-api-rollback-and-a-native-rollback-leave-no-trace-on-aurora-postgresql.

Cause 2 of 2: RestoreDBClusterFromSnapshot answers NotImplemented, so Data API rows cannot be shown to survive a cluster snapshot restore.
AWS: RestoreDBClusterFromSnapshot creates a new cluster from a cluster snapshot and answers 200 with the new DBCluster; its modeled failures are state/quota/not-found faults, never a 'not implemented' code.
Emulator: 'An error occurred (NotImplemented) when calling the RestoreDBClusterFromSnapshot operation: operation RestoreDBClusterFromSnapshot is scaffolded but not implemented in this emulator'. An auto-filed issue already names this stub for check rds.cluster-snapshot-restore; this check is a second, unmarked occurrence - the fixer should close them together rather than twice.
Why: an SDK cannot classify NotImplemented, and no snapshot-restore flow (the standard clone/recover path) can run.
Fix: the RestoreDBClusterFromSnapshot handler in the RDS service (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/restore_db_cluster_from_snapshot.rs).
Traces: rds__rds-data__secretsmanager.data-api-rows-survive-cluster-snapshot-restore.

## Evidence
Cause 1: psycopg.errors.UndefinedTable: no such table: sim2real_dapi_failover; psycopg.errors.UndefinedTable: no such table: sim2real_rollback_both_ways_pg - each after the Data API created and wrote that table on the same cluster.
Cause 2: ClientError (NotImplemented) when calling RestoreDBClusterFromSnapshot: operation RestoreDBClusterFromSnapshot is scaffolded but not implemented in this emulator.

## Steps to Reproduce
Cause 1: create an Aurora PostgreSQL cluster with the Data API enabled and a managed secret; ExecuteStatement 'CREATE TABLE t ...' and an INSERT; then connect natively with the secret's credentials and SELECT from t.
Cause 2: create a cluster snapshot and call RestoreDBClusterFromSnapshot.
