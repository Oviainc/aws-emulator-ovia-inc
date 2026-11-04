Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.get-cluster-credentials-temporary-scoped-access redshift.redshift-sql-connectivity redshift.redshift-native-sql-null-into-a-not-null-column-rejected-and-the-row-absent redshift.redshift-native-sql-rollback-leaves-only-the-committed-rows redshift.rsql-ddl-dml-readback-visible-to-a-second-session redshift.rsql-distkey-changes-the-join-plan redshift.rsql-not-null-violation-rejected redshift.rsql-transaction-rollback-leaves-no-row redshift.redshift-get-cluster-credentials-outside-the-requested-dbgroup-is-denied-at-the-sql-layer
Issue-Key: redshift.get-cluster-credentials-temporary-scoped-access|_pg_connect#20|OperationalError|conn = psycopg.connect(host=host, port=int(port), user=user, password=password,
Issue-Key: redshift.redshift-sql-connectivity|_redshift_sql_roundtrip#15|Timeout|time.sleep(2)
Issue-Key: redshift.redshift-native-sql-null-into-a-not-null-column-rejected-and-the-row-absent|_connect#6|Timeout|time.sleep(interval_s)
Issue-Key: redshift.redshift-native-sql-rollback-leaves-only-the-committed-rows|_connect#6|Timeout|time.sleep(interval_s)
Issue-Key: redshift.rsql-ddl-dml-readback-visible-to-a-second-session|_connect#6|Timeout|time.sleep(2)
Issue-Key: redshift.rsql-distkey-changes-the-join-plan|_connect#8|Timeout|time.sleep(2)
Issue-Key: redshift.rsql-not-null-violation-rejected|_connect#6|Timeout|time.sleep(2)
Issue-Key: redshift.rsql-transaction-rollback-leaves-no-row|_connect#8|Timeout|time.sleep(2)
Issue-Key: redshift.redshift-get-cluster-credentials-outside-the-requested-dbgroup-is-denied-at-the-sql-layer|_run_sql#21|AssertionError|assert described["Status"] == "FINISHED", (
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# Redshift publishes a cluster endpoint but serves no PostgreSQL wire data plane: every native-SQL session is refused or hangs, and the Data API engine cannot parse CREATE GROUP (2 causes)

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusters

## Description
Two net-new root causes about the cluster data plane. Eight behavioral checks never get a SQL session at all; a ninth gets one through the Data API and is refused on a core Redshift DDL statement.

Cause 1 of 2 — the endpoint an available cluster publishes accepts no PostgreSQL-protocol connection. AWS: Amazon Redshift speaks the PostgreSQL wire protocol; once DescribeClusters reports ClusterStatus 'available' with Endpoint.Address and Endpoint.Port (default 5439), a client can open a TCP session to that address/port and authenticate as MasterUsername against DBName, then run SQL (Smithy: Cluster.Endpoint carries Address and Port; the wire data plane itself is not modeled — recorded behavior only: a single-node cluster accepts TCP on the published port shortly after it reports 'available'). Emulator: the control plane reports 'available' and publishes Endpoint.Address/Port, but nothing listens there — a connect attempt is answered with TCP 'Connection refused' (psycopg.OperationalError: connection failed: connection to server at the published host, port 5439 failed: Connection refused), and checks that retry the connect inside their warm-up budget never obtain a session and are killed by the harness ceiling. Why: the entire native data plane is unreachable — DDL/DML round-trips, transaction isolation and rollback semantics, NOT NULL enforcement, distribution-key-dependent query plans and credential-scoped logins can none of them be exercised by any PostgreSQL client or psql, even though the control plane claims a usable cluster. A client that creates a cluster and connects to the endpoint it was given fails at the socket. Fix: the cluster data plane — the service must bind a PostgreSQL-wire listener for the endpoint it publishes (address/port taken from the cluster's own Endpoint) and authenticate the cluster's admin credentials against its DBName; the endpoint must not be advertised before it answers. Traces: redshift.get-cluster-credentials-temporary-scoped-access (explicit 'Connection refused' on the published port), redshift.redshift-sql-connectivity, redshift.redshift-native-sql-null-into-a-not-null-column-rejected-and-the-row-absent, redshift.redshift-native-sql-rollback-leaves-only-the-committed-rows, redshift.rsql-ddl-dml-readback-visible-to-a-second-session, redshift.rsql-distkey-changes-the-join-plan, redshift.rsql-not-null-violation-rejected, redshift.rsql-transaction-rollback-leaves-no-row (all spend their connect budget against the published endpoint and never get a session).

Cause 2 of 2 — the cluster SQL engine cannot parse CREATE GROUP. AWS: CREATE GROUP <name> is core Redshift DDL; executed on a cluster it succeeds (statement FINISHED) and the group becomes grantable so that GetClusterCredentials DbGroups membership can be enforced at the SQL layer (recorded behavior only; the statement is not part of any Smithy shape). Emulator: the statement FAILS with the engine's own parser text 'near "GROUP": syntax error in CREATE GROUP <name> at offset 7', so the dbgroup-scoped access check can never reach its subject. Why: database-group-scoped credentials cannot be modelled at all — a client that provisions groups before vending scoped credentials fails at the first DDL, and the authorization behavior that depends on groups is unobservable. Fix: the cluster SQL engine's statement parser/executor must accept Redshift's CREATE GROUP (and the GRANT path that depends on it). Traces: redshift.redshift-get-cluster-credentials-outside-the-requested-dbgroup-is-denied-at-the-sql-layer. Related: 20261024T024248Z-redshift-cluster-sql-engine-rejects-core-redshift-statements.md and 20261029T193810Z-redshift-cluster-sql-engine-multi-statement-sql-refused.md record other unparsed statements (COPY, UNLOAD, CREATE EXTERNAL SCHEMA, SET, SHOW, ENCODE, INTERVAL) but neither carries CREATE GROUP or this check.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator.

Cause 1, direct socket evidence — redshift.get-cluster-credentials-temporary-scoped-access: "File check_redshift_get_cluster_credentials_temporary_scoped_access.py, line 245, in _pg_connect ... psycopg.OperationalError: connection failed: connection to server at \"127.0.0.1\", port 5439 failed: Connection refused". The host/port are the ones the emulator itself published in Cluster.Endpoint for a cluster it reported 'available'.

Cause 1, the remaining seven checks each (a) create a cluster, (b) assert DescribeClusters reports 'available' with a non-empty Endpoint.Address and a valid Endpoint.Port, then (c) retry psycopg.connect(host=Endpoint.Address, port=Endpoint.Port, user=<admin>, dbname='devdb') inside a 300–600 s warm-up budget. All seven never obtain a session and end as "Error: Check timed out after 60.0s": redshift.redshift-sql-connectivity, redshift.redshift-native-sql-null-into-a-not-null-column-rejected-and-the-row-absent, redshift.redshift-native-sql-rollback-leaves-only-the-committed-rows, redshift.rsql-ddl-dml-readback-visible-to-a-second-session, redshift.rsql-distkey-changes-the-join-plan, redshift.rsql-not-null-violation-rejected, redshift.rsql-transaction-rollback-leaves-no-row. Each check imports its driver before the first create precisely so a missing client would fail as a runner error rather than a target mismatch, and each documents that a post-budget failure is the data plane not being there.

Cause 2: redshift.redshift-get-cluster-credentials-outside-the-requested-dbgroup-is-denied-at-the-sql-layer — "DATAPLANE_UNIMPLEMENTED: SQL 'CREATE GROUP <group>' did not FINISH: status='FAILED' error='near \"GROUP\": syntax error in CREATE GROUP <group> at offset 7'".

Host 127.0.0.1, port 5439, cluster ids and group names above are this run's environment; the requirement is that the published Endpoint.Address/Port accept a PostgreSQL-wire session for the cluster's admin credential, and that CREATE GROUP parse and FINISH.

## Steps to Reproduce
1. CreateCluster (single-node, DBName 'devdb') and poll DescribeClusters until ClusterStatus is 'available'; note Endpoint.Address and Endpoint.Port.
2. Open a PostgreSQL connection to that address/port with the cluster's MasterUsername/MasterUserPassword and dbname 'devdb' (psycopg or psql): the connection is refused, and retrying for the full warm-up budget never succeeds.
3. Through the Data API on the same cluster, run 'CREATE GROUP <name>' and observe status FAILED with 'near "GROUP": syntax error ... at offset 7'.
4. Re-replay the behavioral checks listed in trace_ids.
