Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dsql.native-sql-round-trip-create-insert-select dsql.connection-without-tls-or-admin-token-rejected dsql.expired-auth-token-rejected-open-session-survives dsql.iam-mapped-database-role-privileges-denied dsql.dsql-occ-write-conflict-rejects-the-losing-commit dsql.locking-read-conflicts-rejected-at-commit dsql.transaction-constraints-rejected dsql.unsupported-postgres-statements-rejected dsql.sequence-and-identity-cache-rule-rejected
Issue-Shape-Id: com.amazonaws.dsql#DSQL
Issue-Key: dsql.native-sql-round-trip-create-insert-select|_connect_with_retry#12|Timeout|time.sleep(interval_s)
Issue-Key: dsql.connection-without-tls-or-admin-token-rejected|_connect_ready#5|Timeout|time.sleep(5)
Issue-Key: dsql.expired-auth-token-rejected-open-session-survives|_connect_ready#4|Timeout|time.sleep(5)
Issue-Key: dsql.iam-mapped-database-role-privileges-denied|_connect_ready#13|Timeout|time.sleep(5)
Issue-Key: dsql.dsql-occ-write-conflict-rejects-the-losing-commit|_connect_with_retry#7|Timeout|time.sleep(interval_s)
Issue-Key: dsql.locking-read-conflicts-rejected-at-commit|_connect_ready#6|Timeout|time.sleep(5)
Issue-Key: dsql.transaction-constraints-rejected|_connect_ready#5|Timeout|time.sleep(5)
Issue-Key: dsql.unsupported-postgres-statements-rejected|_connect_ready#5|Timeout|time.sleep(5)
Issue-Key: dsql.sequence-and-identity-cache-rule-rejected|_connect_ready#6|Timeout|time.sleep(5)
Issue-Commit: 982a07ada62dfbeb289a85399a86f8ade68b0ada

# DSQL clusters report ACTIVE with a published endpoint but no PostgreSQL wire listener answers it, so all nine native-SQL checks time out at connect

> _Note: this issue was identified against the code base at commit `982a07ada62dfbeb289a85399a86f8ade68b0ada`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Cause 1 of 1: the cluster endpoint the control plane publishes accepts no PostgreSQL wire connection, so the whole Aurora DSQL data plane is unreachable. Failing checks: dsql.native-sql-round-trip-create-insert-select, dsql.connection-without-tls-or-admin-token-rejected, dsql.expired-auth-token-rejected-open-session-survives, dsql.iam-mapped-database-role-privileges-denied, dsql.dsql-occ-write-conflict-rejects-the-losing-commit, dsql.locking-read-conflicts-rejected-at-commit, dsql.transaction-constraints-rejected, dsql.unsupported-postgres-statements-rejected, dsql.sequence-and-identity-cache-rule-rejected.

NOTE for triage: the open issue 20261022T193920Z-dsql-publishes-a-cluster-endpoint-but-serves-no.md describes this same root cause and names the same nine checks; the replay report did not mark these checks as owned by it, so they are re-filed here with their replay keys bound. Close the two together.

AWS: recorded behavior only — the Smithy model (services/service-com-amazonaws-dsql-dsql/smithy-model/com-amazonaws-dsql-dsql.json) is control-plane only; it binds no SQL-carrying operation and exposes the endpoint merely as GetCluster/CreateCluster's optional `endpoint` member (Endpoint, pattern ^[a-zA-Z0-9.-]+$). The recordings show that published endpoint is a live PostgreSQL server: once a cluster reports ACTIVE, a libpq client connects to it on TCP 5432, database `postgres`, user `admin`, with a DSQL IAM auth token as the password (GenerateDbConnectAuthToken / GenerateDbConnectAdminAuthToken are client-side presigners needing no server support) and gets a working session in which SELECT current_user, 1 + 1 returns ('admin', 2). The recordings also show the endpoint enforces data-plane rules: TLS is mandatory (plaintext refused, recorded FATAL text as an example: "unable to accept connection, SSL is mandatory. AWS Authentication is required."); a token whose action or host does not match is refused (example FATAL "unable to accept connection, access denied", with HINT "Wrong user to action mapping. user: admin, action: DbConnect" for a DbConnect token presented as admin); sessions are REPEATABLE READ with optimistic concurrency that rejects the losing commit; and DSQL's unsupported PostgreSQL features are refused with SQLSTATE 0A000.

Emulator: the control plane mints and stores an endpoint — services/service-com-amazonaws-dsql-dsql/src/state.rs cluster_endpoint() renders {identifier}.dsql.{region}.on.aws and CreateCluster persists it, GetCluster returns it and settles the cluster to ACTIVE — but nothing in the service speaks the PostgreSQL wire protocol at it: services/service-com-amazonaws-dsql-dsql/src/lib.rs declares only ops, scaffolded, service and state, with no data-plane listener module. Every check that resolves the endpoint and opens a libpq connection retries a connection that is never accepted and is killed by the harness: all nine dp:native / plane:data checks report "Check timed out after 60.0s" inside their connect-retry loops (_connect_ready / _connect_with_retry, budgets 180 s / 120 s), so none of their SQL assertions is ever reached. This is not a missing runner tool — psycopg is importable in the runner (the native-SQL check raises RequiredToolMissing when it is not, and it did not) — and no assertion involved depends on a live account value. The endpoint it announces is also not an address the emulator is reached on.

Why: a cluster that reports ACTIVE and advertises an endpoint but accepts no connection is the most misleading state possible for a client. The normal application flow (CreateCluster, wait for ACTIVE, generate an auth token, connect, run SQL) hangs on connect instead of failing fast, and every behavior a DSQL client depends on — mandatory TLS, auth-token validation scoped to action and host, token expiry not killing an open session, IAM-mapped database role privileges, REPEATABLE READ + OCC commit rejection, the 0A000 refusal of unsupported statements, the sequence/identity CACHE rule, and the DDL/DML transaction constraints — is unimplemented and unverifiable, while the target still looks healthy to anything grading only control-plane envelopes.

Fix: the service needs a data-plane component — a PostgreSQL wire-protocol listener reached at the address the control plane publishes for a cluster (port 5432), mounted alongside the control-plane service module, which requires TLS, authenticates the password as a DSQL auth token validated against the connecting cluster's host and the requested action/user, and executes SQL under DSQL's documented semantics; and the published endpoint must be an address the emulator actually answers on. Until a listener exists, none of the per-statement semantics above can be reached.

Traces: the nine failing checks listed above, every one timing out at the connect step. File citations: services/service-com-amazonaws-dsql-dsql/src/lib.rs lines 10-17 (module list, no data plane); services/service-com-amazonaws-dsql-dsql/src/state.rs cluster_endpoint (around line 780); services/service-com-amazonaws-dsql-dsql/src/ops/get_cluster.rs lines 181-199 (endpoint reported with status ACTIVE).

## Evidence
Expected (recorded AWS): after GetCluster reports status=ACTIVE with an `endpoint`, psycopg.connect(host=<endpoint>, port=5432, dbname="postgres", user="admin", password=<admin auth token>, sslmode="require") opens a session where SELECT current_user, 1 + 1 returns ('admin', 2); CREATE TABLE / INSERT / SELECT / DROP TABLE round-trips; a plaintext (sslmode=disable) attempt is refused (example FATAL "unable to accept connection, SSL is mandatory. AWS Authentication is required."); a DbConnect token used for admin, a non-token password and an admin token signed for another cluster's host are each refused (example FATAL "unable to accept connection, access denied"); unsupported statements are refused with SQLSTATE 0A000.

Actual (emulator): the control plane answers (CreateCluster returns identifier/arn, GetCluster settles to ACTIVE and returns an endpoint of the form <identifier>.dsql.<region>.on.aws), but no connection to that endpoint on 5432 is ever accepted. All nine data-plane checks report exactly:
  Error: Check timed out after 60.0s
with the failure occurring inside the connect-retry loop, so no SQL-level assertion is reached. The crate confirms the absence: services/service-com-amazonaws-dsql-dsql/src/lib.rs exposes only `ops`, `scaffolded`, `service`, `state` — there is no PostgreSQL listener behind the advertised endpoint.

## Steps to Reproduce
1. CreateCluster(deletionProtectionEnabled=false).
2. Poll GetCluster(identifier=<id>) until status == "ACTIVE"; note the returned `endpoint`.
3. GenerateDbConnectAdminAuthToken(Hostname=<endpoint>, Region=<region>, ExpiresIn=900) (client-side presigner).
4. Connect a PostgreSQL client to that endpoint: psycopg.connect(host=<endpoint>, port=5432, dbname="postgres", user="admin", password=<token>, sslmode="require", connect_timeout=20).
5. The connection is never accepted — the client retries until the check budget expires — where AWS opens a session in which SELECT current_user, 1 + 1 returns ('admin', 2).
6. Consequently none of the data-plane behaviors can be probed: plaintext refusal, auth-token action/host validation, expiry-vs-open-session, IAM-mapped role privileges, REPEATABLE READ + OCC commit rejection, SQLSTATE 0A000 refusals, the sequence/identity CACHE rule and the DDL/DML transaction constraints all remain unreachable.
