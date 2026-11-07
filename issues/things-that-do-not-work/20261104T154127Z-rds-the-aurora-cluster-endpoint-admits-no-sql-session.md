Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.deep-rds-1-rds-engine-truth-aurora-postgres-constraint-and-lock-violations-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.deep-rds-1-rds-engine-truth-aurora-postgres-constraint-and-lock-violations-rejected|_retry_delete#6|Timeout|time.sleep(interval_s)
Issue-Commit: ed43c7c5e13b0f1212f82cf25816ea1e565f9817

# rds: an Aurora PostgreSQL cluster endpoint admits no SQL session (split out of the two-cause report)

> _Split out of `issues/things-that-now-work/20261104T154127Z-rds-2-net-new-causes-the-az-coverage.md`, whose cause 1 (the AZ-coverage refusal) is fixed. The cause below is NOT fixed; it keeps the original report's numbering, trace id and Issue-Key so it stays filed and replayable. It was identified against the code base at commit `ed43c7c5e13b0f1212f82cf25816ea1e565f9817`, so trace it back to the current code base._

## Operation
CreateDBCluster

## Description
Cause 2 of 2: an Aurora PostgreSQL cluster never reaches a state in which a SQL session can be opened on the endpoint it publishes, so the engine-truth check cannot execute a single statement.

AWS: `CreateDBCluster(Engine='aurora-postgresql')` plus a writer `CreateDBInstance` in that cluster converge to Status/DBInstanceStatus 'available' and publish `DBCluster$Endpoint` with `DBCluster$Port` 5432; a client that opens a TLS PostgreSQL session to that address:port with MasterUsername/MasterUserPassword against DatabaseName then observes PostgreSQL engine behavior — SQLSTATE 23505 on a UNIQUE violation naming the constraint, 23503 on a foreign-key violation, 55P03 `lock_not_available` for `FOR UPDATE NOWAIT` on a row another transaction holds, 40P01 `deadlock_detected` for exactly one victim of a two-session lock cycle, and `ON DELETE CASCADE` removing only the deleted parent's dependents. Smithy: `CreateDBCluster` output `DBCluster$Endpoint`/`$ReaderEndpoint`/`$Port`/`$Status`, `DBCluster$MasterUsername`, `DBCluster$DatabaseName`; the SQLSTATE values are recorded behavior only (PostgreSQL error-code appendix, cited by the check).

Emulator: no listener is opened for a DB CLUSTER at all — `src/state/data_plane.rs::engine_wire` serves the instance-level engines (`postgres`, `mysql`, `mariadb`) only, and a cluster publishes the DNS-form `<id>.cluster-<label>.<region>.rds.amazonaws.com` that nothing answers on. The check therefore spends its budget in the first-connect poll and times out inside its teardown's retry loop (`_retry_delete`), with no response to compare.

Why: a client that does the one thing an Aurora cluster exists for — connect and run SQL — gets nothing: no constraint refusal, no lock/deadlock semantics, no cascade behavior, and no way to tell from the control plane that the data plane is absent.

Traces: rds.deep-rds-1-rds-engine-truth-aurora-postgres-constraint-and-lock-violations-rejected (mismatch, timeout at the connect/teardown poll).

## What is known

* The CONTROL plane is not what stalls. `CreateDBCluster` for `aurora-postgresql` and `aurora-mysql` plus a writer `CreateDBInstance` both converge to `available` without any mutating call, the cluster publishes both endpoints, the engine's port, the master user and the database the create named, a cluster that still holds a member refuses its own delete, and the teardown then completes. That sequence is pinned by `services/service-com-amazonaws-rds-amazonrdsv19/tests/all/scratch_aurora_truth.rs`, so a fixer need not re-derive it.
* The wire and the engine a cluster endpoint would need already exist for DB INSTANCES: `src/state/data_plane/` holds the PostgreSQL and MySQL server halves, the in-process SQLite engine per database, the session admission (master-user digests, `rds.force_ssl`), and `open_engine_endpoint`, which the core binds the socket and the TLS for. A cluster-level endpoint is that machinery keyed on the cluster record rather than on an instance record.
* THE BLOCKER is the port, and it is in the read-only core. `Listeners::tcp_plaintext` (vendor/server/src/listeners.rs:265, and `tcp`/`tcp_with_idle` beside it) takes no requested port: it binds in the core's own range (`EMULATOR_LISTENER_PORT_RANGE`, OS-assigned when unset) on the single bind address the core owns. A served cluster endpoint could therefore only publish an ephemeral port, while the recorded client dials the engine's documented `DBCluster$Port` 5432 — so opening the listener would announce an address that client cannot reach and would make `DBCluster$Port` diverge for every Aurora cluster. The same gap is recorded for a DB instance in `issues/things-that-do-not-work/20261105T090000Z-rds-a-db-instance-publishes-the-listeners-port-not-the-requested-port.md`, and the cluster endpoint's absence for the other 58 checks in `issues/things-that-do-not-work/20261104T053605Z-rds-the-published-db-instance-cluster-endpoint-serves.md`.
  Required core change: `Listeners::tcp_plaintext` (and the TLS variants) should accept an optional port to bind — and, so two clusters can each serve the engine's port, an optional loopback address — answering the service's own "port in use" case when it cannot, after which `open_engine_endpoint` can ask for the cluster's `Port` and the control plane can publish the value it serves on.
* A SECOND gap, independent of the port: the check's lock beats need two sessions holding independent transactions (55P03 for `FOR UPDATE NOWAIT`, 40P01 for exactly one deadlock victim), and the engine behind an endpoint is one `rusqlite` connection per database shared by every session on it (`src/state/data_plane/sql_engine.rs`: "The sessions on one database share its connection"). Row-level locking and deadlock detection across sessions need per-session transactions over one shared store — a redesign of that engine — so those SQLSTATEs cannot be produced faithfully today either. The UNIQUE and foreign-key SQLSTATEs (23505, 23503) and `ON DELETE CASCADE` the check also asserts are already served by that engine.

## Steps to Reproduce
1. `rds.create_db_cluster(DBClusterIdentifier=<id>, Engine='aurora-postgresql', EngineVersion=<default>, MasterUsername='sim2real', MasterUserPassword=<pw>, DatabaseName='enginetruth', DBSubnetGroupName=<sng>, VpcSecurityGroupIds=[<sg>], BackupRetentionPeriod=1)`.
2. `rds.create_db_instance(DBInstanceIdentifier=<id>-w, DBClusterIdentifier=<id>, Engine='aurora-postgresql', DBInstanceClass=<orderable small class>, PubliclyAccessible=False)`.
3. Poll `describe_db_instances` / `describe_db_clusters` for 'available', then read `DBCluster.Endpoint` and `DBCluster.Port` (expect 5432).
4. Open a PostgreSQL session to that address:port with the master credentials and dbname 'enginetruth' (psycopg, `sslmode='require'`) and run `SELECT 1` — nothing is listening on the published address, so no session is ever established.
