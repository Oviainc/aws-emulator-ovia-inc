Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dsql.native-sql-round-trip-create-insert-select dsql.create-stream-lifecycle-to-kinesis
Issue-Shape-Id: com.amazonaws.dsql#DSQL
Issue-Key: dsql.native-sql-round-trip-create-insert-select|_connect_with_retry#12|AssertionError|raise AssertionError(f"could not open a SQL connection within {timeout_s}s: {last_exc}")
Issue-Key: dsql.create-stream-lifecycle-to-kinesis|_connect_with_retry#34|AssertionError|raise AssertionError(f"could not open a Postgres wire connection to {endpoint}: {last_err}")
Issue-Commit: ae442358c61414b2bbb5a7e8dd80b7eec77accf8

# dsql: the published cluster endpoint accepts no PostgreSQL wire connection, so the native-SQL round-trip and the cluster+CDC-stream deploy workflow both dead-end at connect

> _Note: this issue was identified against the code base at commit `ae442358c61414b2bbb5a7e8dd80b7eec77accf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Cause 1 of 1: a DSQL cluster settles to ACTIVE and publishes an `endpoint`, but nothing in the emulator speaks the PostgreSQL wire protocol at that address — a libpq connect to it on TCP 5432 is refused outright. Failing checks: dsql.native-sql-round-trip-create-insert-select, dsql.create-stream-lifecycle-to-kinesis.

NOTE for triage: two open issues describe this same root cause and should be closed together with this one — 20261023T121409Z-dsql-clusters-report-active-with-a-published-endpoint.md (which owns nine other native-SQL checks and already lists dsql.native-sql-round-trip-create-insert-select in its trace ids) and 20261022T193920Z-dsql-publishes-a-cluster-endpoint-but-serves-no.md. The replay report did not mark these two checks as owned by either, so they are re-filed here with the failing check ids they must be re-replayed against. The fixed issue 20261022T152600Z-dsql-aurora-dsql-9-behavioral-divergences-no-postgresql-data.md also claimed this data plane; it is still absent in the current checkout. The CDC check additionally never reaches its Kinesis-delivery assertions, which the open issue 20261024T074208Z-dsql-createstream-event-propagation.md covers; this issue is only about the connect step that blocks it.

AWS: recorded behavior only — the Smithy model (services/service-com-amazonaws-dsql-dsql/smithy-model/com-amazonaws-dsql-dsql.json) is control-plane only and binds no SQL-carrying operation; it exposes the endpoint solely as CreateCluster/GetCluster's optional `endpoint` member (shape Endpoint, pattern ^[a-zA-Z0-9.-]+$). The recordings show that endpoint is a live PostgreSQL server: once GetCluster reports status ACTIVE, a libpq client connects to the published host on TCP 5432, database `postgres`, user `admin`, with a DSQL IAM auth token as the password (GenerateDbConnectAuthToken / GenerateDbConnectAdminAuthToken are client-side presigners that need no server support), and gets a working session that round-trips CREATE TABLE / INSERT / SELECT / DROP TABLE and, for the CDC workflow, produces change records the configured Kinesis target receives. The connection itself succeeds; no error code or status is recorded for it.

Emulator: the control plane mints and persists an endpoint of the form <identifier>.dsql.<region>.on.aws (services/service-com-amazonaws-dsql-dsql/src/state.rs cluster_endpoint()) and GetCluster returns it with status ACTIVE, but no listener exists behind it: services/service-com-amazonaws-dsql-dsql/src/lib.rs (lines 10-13) declares only `ops`, `scaffolded`, `service` and `state` — there is no data-plane module. Both checks therefore fail in their connect-retry loops with the libpq error, verbatim: `connection failed: connection to server at "127.0.0.1", port 5432 failed: Connection refused` / `Is the server running on that host and accepting TCP/IP connections?` — raised as `AssertionError: could not open a SQL connection within 120.0s` and `AssertionError: could not open a Postgres wire connection to <host>`. The fixed text of that message is libpq's; the host and port are the address the emulator published for the cluster, so they vary per environment and are not part of the requirement. This is not a missing runner tool — the native-SQL check raises RequiredToolMissing when psycopg is unavailable and it did not — and no assertion reached depends on a live-account value.

Why: a cluster that reports ACTIVE and advertises an endpoint it does not answer on is the most misleading state a client can be handed. The standard application flow (CreateCluster, wait for ACTIVE, presign an auth token, connect, run SQL) fails at connect, so no SQL work can be done against the emulator at all; and the whole deploy workflow that writes a row over the wire and expects it to surface as a CDC change record on a Kinesis stream is unreachable, even though every control-plane envelope in it (CreateCluster, PutClusterPolicy/GetClusterPolicy, CreateStream, tagging) answers successfully. Anything grading only control-plane responses sees a healthy target.

Fix: the service crate needs a data-plane component — a PostgreSQL wire-protocol listener mounted alongside the control-plane service module and reached at exactly the address state.rs cluster_endpoint() publishes (port 5432), which authenticates the password as a DSQL auth token scoped to the connecting cluster's host and the requested action/user and executes SQL under DSQL's semantics; and the endpoint CreateCluster/GetCluster report must be an address the emulator actually accepts connections on. Writes committed through it must also feed the CDC stream machinery so change records reach the stream's Kinesis target.

Traces: dsql.native-sql-round-trip-create-insert-select (fails at check_dsql_native_sql_round_trip_create_insert_select.py line 46 -> _connect_with_retry line 118) and dsql.create-stream-lifecycle-to-kinesis (fails at check_dsql_create_stream_lifecycle_to_kinesis.py line 98 -> _connect_with_retry line 226). File citations: services/service-com-amazonaws-dsql-dsql/src/lib.rs lines 10-13 (module list, no data plane); services/service-com-amazonaws-dsql-dsql/src/state.rs cluster_endpoint(); services/service-com-amazonaws-dsql-dsql/src/ops/get_cluster.rs (endpoint reported together with status ACTIVE).

## Evidence
Expected (recorded AWS): after GetCluster returns status="ACTIVE" and an `endpoint`, psycopg.connect(host=<endpoint>, port=5432, dbname="postgres", user="admin", password=<admin auth token from GenerateDbConnectAdminAuthToken>, sslmode="require") opens a session; CREATE TABLE <t> (id INTEGER PRIMARY KEY, val TEXT) / INSERT / SELECT val FROM <t> WHERE id = 1 round-trips and returns the inserted value; and in the deploy workflow a row written over that session arrives as a change record on the stream's Kinesis target.

Actual (emulator): CreateCluster, GetCluster (status ACTIVE + endpoint), PutClusterPolicy/GetClusterPolicy, tagging and CreateStream all succeed, but every connection attempt to the published endpoint on 5432 is refused at TCP level. Both checks end in their retry loops:
  dsql.native-sql-round-trip-create-insert-select — AssertionError: could not open a SQL connection within 120.0s: connection failed: connection to server at "127.0.0.1", port 5432 failed: Connection refused / Is the server running on that host and accepting TCP/IP connections?
  dsql.create-stream-lifecycle-to-kinesis — AssertionError: could not open a Postgres wire connection to 127.0.0.1: connection failed: connection to server at "127.0.0.1", port 5432 failed: Connection refused / Is the server running on that host and accepting TCP/IP connections?
No SQL-level or CDC-delivery assertion is ever reached. The crate confirms the cause: services/service-com-amazonaws-dsql-dsql/src/lib.rs exposes only ops, scaffolded, service, state — there is no PostgreSQL listener behind the advertised endpoint.

## Steps to Reproduce
1. CreateCluster(deletionProtectionEnabled=false, tags={...}).
2. Poll GetCluster(identifier=<id>) until status == "ACTIVE"; note the returned `endpoint`.
3. GenerateDbConnectAdminAuthToken(Hostname=<endpoint>, Region=<region>, ExpiresIn=900) (client-side presigner, no server call).
4. Connect a PostgreSQL client to that endpoint: psycopg.connect(host=<endpoint>, port=5432, dbname="postgres", user="admin", password=<token>, sslmode="require", connect_timeout=20).
5. Observe the connection is refused immediately ("Connection refused") and keeps being refused until the check's 120 s budget expires, where AWS opens a working session.
6. For the deploy path, additionally PutClusterPolicy + CreateStream with a Kinesis target first: all control-plane calls succeed, then the same connect at step 4 is refused, so the row-write and the Kinesis change-record assertion are never reached.
