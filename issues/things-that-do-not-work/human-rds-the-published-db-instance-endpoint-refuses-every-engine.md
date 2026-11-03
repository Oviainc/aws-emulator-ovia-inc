# rds: the published DB instance endpoint refuses every engine connection — 17 PostgreSQL/MySQL connection-semantics checks never open a session and are killed at the 60s harness cap

> _Note: this issue was identified against the code base at commit `adb5576e5b4c8de09688914f068f290c40dc2de8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstance

## Description
Cause 1 of 1: the endpoint RDS publishes for a DB instance accepts no engine session — the TCP connect is refused.
AWS: recorded behavior only for the data plane (the Smithy control-plane model describes the endpoint, not the wire protocol). CreateDBInstance returns Endpoint{Address, Port} (5432 for postgres, 3306 for mysql); once DescribeDBInstances reports DBInstanceStatus='available', that address:port serves the engine's native protocol — a PostgreSQL/MySQL client can complete a startup handshake, authenticate as MasterUsername against DBName, and run SQL; a wrong password or an unknown database is refused by the engine (not by the socket); and TLS is negotiated in band. For PostgreSQL the recordings show the in-band negotiation directly: a libpq client with sslmode=prefer sends SSLRequest in the clear, the server answers 'S', and the session is upgraded to TLS before the startup message — psycopg reports pgconn.ssl_in_use is True, `SELECT ssl, version FROM pg_stat_ssl WHERE pid = pg_backend_pid()` returns one row with ssl = true and version TLSv1.2 or TLSv1.3, `SHOW ssl` returns ('on',), and `SELECT 6 * 7` returns (42,) on that session. Under the default parameter group a plaintext (sslmode=disable) PostgreSQL login is refused by the server, while a default MySQL instance admits a plaintext login and reports a cipher on a TLS session.
Emulator: the address is published and the instance reports 'available', but nothing listens: a native connect is answered `Connection refused`. services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs `endpoint_address` (around line 2478) renders the published address and src/ops/create_db_instance.rs (around line 523) assigns the port from `crate::state::default_port` (5432 for postgres, around src/state.rs line 5719), but src/lib.rs (lines 10-13) declares only `ops`, `scaffolded`, `service` and `state` — there is no data-plane listener module, and no code in the crate calls the core's listener API. The one check that surfaces it directly raises `DATAPLANE_UNIMPLEMENTED: could not open the admin postgres session ... Connection refused`. Every other connection-semantics check retries the connect and is killed at the 60s harness cap instead of reaching an outcome. Because no session can be opened, none of the engine-level contracts is observable: in-band TLS negotiation (sslmode=prefer) and pg_stat_ssl reporting, plaintext-vs-TLS admission, require_secure_transport / ssl enforcement, server version vs. the described EngineVersion, session identity (current_user / current_database), unknown-database and wrong-password refusals, SUPER / superuser privilege refusals, dialect behaviour (numeric, timestamptz, jsonb, arrays, RETURNING, recursive CTE), auto_increment / utf8mb4 / strict-mode overflow, the custom-port contract, IAM-auth-token identity binding, reboot severing an open session, and a stopped instance refusing SQL.
Why: any client that actually talks to the database — every application under test, every migration, every ORM — cannot connect at all, while the control plane claims the instance is available. No open issue in this repository cites any of the checks listed under Trace pointers.
Fix: belongs in the RDS data plane — a per-instance engine listener bound to the published Endpoint.Address/Port that receives the connection before any TLS is terminated (PostgreSQL sends SSLRequest and MySQL sends its greeting in the clear, and both upgrade to TLS in band, so a listener that is TLS from the first byte cannot serve either), completes the server-side TLS handshake after the in-band request, authenticates MasterUsername/MasterUserPassword against DBName, and executes SQL; plus the instance state that marks an instance 'available' only once it serves, and that closes the listener on delete.
Traces: rds.iam-db-auth-token-minted-for-another-user-is-refused (explicit connection-refused), rds.mysql-server-version-matches-the-described-engine-version, rds.mysql-wrong-master-password-login-rejected, rds.mysql-plaintext-login-admitted-by-default-and-tls-session-reports-a-cipher, rds.mysql-require-secure-transport-plaintext-login-rejected, rds.mysql-master-user-lacks-super-set-global-rejected, rds.mysql-auto-increment-utf8mb4-and-strict-overflow-rejected, rds.pg-dialect-numeric-timestamptz-jsonb-arrays-returning-recursive-cte, rds.pg-master-user-not-superuser-alter-system-rejected, rds.postgres-session-identity-matches-master-username-and-db-name, rds.postgres-unknown-database-login-rejected, rds.postgres-sslmode-prefer-login-negotiates-tls-in-band, rds.postgres-default-parameter-group-plaintext-login-rejected, rds.postgres-custom-port-is-the-published-and-listening-port, rds.reboot-severs-open-session-rolls-back-uncommitted-keeps-committed, rds.stopped-instance-refuses-sql-and-start-keeps-rows, rds.deferred-class-modification-stays-pending-while-instance-serves-sql.

## Trace pointers
Severity: CRITICAL.
Corpus: behavioral suite `simparity-behavioral-aws-v1`. Service shape: `com.amazonaws.rds#AmazonRDSv19`.
Identified against commit `adb5576e5b4c8de09688914f068f290c40dc2de8`.
The 17 failing checks below are the units to replay to verify a fix. Each is a check id in that suite:

- `rds.iam-db-auth-token-minted-for-another-user-is-refused`
- `rds.mysql-server-version-matches-the-described-engine-version`
- `rds.mysql-wrong-master-password-login-rejected`
- `rds.mysql-plaintext-login-admitted-by-default-and-tls-session-reports-a-cipher`
- `rds.mysql-require-secure-transport-plaintext-login-rejected`
- `rds.mysql-master-user-lacks-super-set-global-rejected`
- `rds.mysql-auto-increment-utf8mb4-and-strict-overflow-rejected`
- `rds.pg-dialect-numeric-timestamptz-jsonb-arrays-returning-recursive-cte`
- `rds.pg-master-user-not-superuser-alter-system-rejected`
- `rds.postgres-session-identity-matches-master-username-and-db-name`
- `rds.postgres-unknown-database-login-rejected`
- `rds.postgres-sslmode-prefer-login-negotiates-tls-in-band`
- `rds.postgres-default-parameter-group-plaintext-login-rejected`
- `rds.postgres-custom-port-is-the-published-and-listening-port`
- `rds.reboot-severs-open-session-rolls-back-uncommitted-keeps-committed`
- `rds.stopped-instance-refuses-sql-and-start-keeps-rows`
- `rds.deferred-class-modification-stays-pending-while-instance-serves-sql`

## Evidence
rds.iam-db-auth-token-minted-for-another-user-is-refused (the only one that raises rather than timing out):
  expected: a native PostgreSQL session on the published Endpoint (Address, Port 5432) as MasterUsername
  actual:   AssertionError: DATAPLANE_UNIMPLEMENTED: could not open the admin postgres session at <endpoint>:5432: OperationalError('connection failed: ... Connection refused\n\tIs the server running on that host and accepting TCP/IP connections?')
rds.postgres-sslmode-prefer-login-negotiates-tls-in-band:
  expected: after DBInstanceStatus='available', psycopg.connect(host=Endpoint.Address, port=Endpoint.Port, user=MasterUsername, password=MasterUserPassword, dbname=DBName, sslmode='prefer') opens a session with pgconn.ssl_in_use True; pg_stat_ssl for the session's own backend reports ssl = true with TLSv1.2/TLSv1.3; SHOW ssl = 'on'; SELECT 6 * 7 = 42.
  actual:   'Error: Check timed out after 60.0s' inside _pg_first's connect-retry loop; no assertion reached.
The other 15 connection-semantics checks listed under Trace pointers each report: 'Error: Check timed out after 60.0s' (each creates an instance, polls DescribeDBInstances for 'available', then connects to the published endpoint).

## Steps to Reproduce
1. Build and boot the emulator from the checkout, then replay the behavioral suite.
2. CreateDBInstance(Engine='postgres', MasterUsername=U, MasterUserPassword=P, DBName=D); poll DescribeDBInstances until DBInstanceStatus=='available'; open a PostgreSQL client with sslmode=prefer against Endpoint.Address:Endpoint.Port — observe 'Connection refused' (the client retries until the check budget expires).
3. Repeat with Engine='mysql' on port 3306 — observe the same refusal.
