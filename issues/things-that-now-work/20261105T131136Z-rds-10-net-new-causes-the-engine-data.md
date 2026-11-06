Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.postgres-custom-port-is-the-published-and-listening-port rds.deploy-db-instance-cfn-behaves rds.engine-dialect-discriminator-query rds.deep-rds-1-rds-engine-truth-mysql-constraint-lock-and-charset-violations-rejected rds.deep-rds-sql-truth-iam-token-past-expiry-rejected rds.deep-rds-sql-truth-mysql-replica-write-rejected-read-only rds.cloudwatch-logs-export-delivers-only-while-enabled rds.log-file-portion-contains-the-statement-the-engine-executed rds.max-connections-cap-rejects-the-overflow-session rds.paramgroup-out-of-range-value-rejected-engine-setting-unchanged rds.serializable-write-skew-rejected-repeatable-read-permits rds.static-parameter-pending-reboot-query-effect rds.postgres-session-identity-matches-master-username-and-db-name rds.postgres-unknown-database-login-rejected rds.pg-master-user-not-superuser-alter-system-rejected rds.postgres-default-parameter-group-plaintext-login-rejected rds.deferred-class-modification-stays-pending-while-instance-serves-sql rds.failover-force-failover-rejected-on-a-single-az-instance-moves-no-write-role rds.reboot-severs-open-session-rolls-back-uncommitted-keeps-committed rds.read-replica-replication-query-and-promote rds.e2e-parameter-change-affects-runtime-behavior rds.pg-dialect-numeric-timestamptz-jsonb-arrays-returning-recursive-cte rds.iam-db-auth-token-connects-and-maps-to-the-granted-role rds.iam-db-auth-token-minted-for-another-user-is-refused rds.copy-from-stdin-bulk-load-rejects-a-malformed-row rds.repeatable-read-transaction-sees-a-stable-snapshot rds.mysql-auto-increment-utf8mb4-and-strict-overflow-rejected rds.mysql-master-user-lacks-super-set-global-rejected rds.mysql-require-secure-transport-plaintext-login-rejected rds.mysql-wrong-master-password-login-rejected rds.delete-instance-with-deletion-protection-rejected-rows-still-queryable rds.modify-master-password-below-policy-rejected-live-credential-unaffected rds.e2e-point-in-time-restore-recovers-data rds.point-in-time-restore-data-query-round-trip rds.snaprestore-restore-over-live-instance-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.postgres-custom-port-is-the-published-and-listening-port|run#6|AssertionError|assert inst["Endpoint"]["Port"] == _PORT, (
Issue-Key: rds.deploy-db-instance-cfn-behaves|run#11|AssertionError|assert endpoint["Port"] == 3306, f"expected port 3306, got {endpoint['Port']!r}"
Issue-Key: rds.engine-dialect-discriminator-query|run#8|AssertionError|assert inst["Endpoint"]["Port"] == port, (
Issue-Key: rds.deep-rds-1-rds-engine-truth-mysql-constraint-lock-and-charset-violations-rejected|run#6|AssertionError|assert port == _PORT, f"mysql must listen on {_PORT}; DescribeDBInstances says {port!r}"
Issue-Key: rds.deep-rds-sql-truth-iam-token-past-expiry-rejected|run#13|AssertionError|assert port == _PORT, f"postgres must listen on {_PORT}; DescribeDBInstances says {port!r}"
Issue-Key: rds.deep-rds-sql-truth-mysql-replica-write-rejected-read-only|run#12|AssertionError|assert src_port == _PORT, f"mysql must listen on {_PORT}; DescribeDBInstances says {src_port!r}"
Issue-Key: rds.cloudwatch-logs-export-delivers-only-while-enabled|_sql_rows#27|UndefinedTable|cur.execute(statement)
Issue-Key: rds.log-file-portion-contains-the-statement-the-engine-executed|_sql_rows#25|UndefinedTable|cur.execute(statement)
Issue-Key: rds.max-connections-cap-rejects-the-overflow-session|_sql_rows#24|UndefinedTable|cur.execute(statement)
Issue-Key: rds.paramgroup-out-of-range-value-rejected-engine-setting-unchanged|_setting#30|UndefinedTable|cur.execute("SELECT setting FROM pg_settings WHERE name = %s", (name,))
Issue-Key: rds.serializable-write-skew-rejected-repeatable-read-permits|_rows#11|UndefinedTable|cur.execute(statement)
Issue-Key: rds.static-parameter-pending-reboot-query-effect|_sql_rows#18|UndefinedTable|cur.execute(statement)
Issue-Key: rds.postgres-session-identity-matches-master-username-and-db-name|_rows#10|UndefinedTable|cur.execute(sql, params)
Issue-Key: rds.postgres-unknown-database-login-rejected|_rows#9|UndefinedTable|cur.execute(sql, params)
Issue-Key: rds.pg-master-user-not-superuser-alter-system-rejected|_rows#5|UndefinedTable|cur = conn.execute(sql, params)
Issue-Key: rds.postgres-default-parameter-group-plaintext-login-rejected|_rows#12|UndefinedTable|cur.execute(sql, params)
Issue-Key: rds.deferred-class-modification-stays-pending-while-instance-serves-sql|_rows#10|UndefinedTable|cur = conn.execute(sql, params)
Issue-Key: rds.failover-force-failover-rejected-on-a-single-az-instance-moves-no-write-role|_sql_rows#20|UndefinedTable|cur.execute(statement)
Issue-Key: rds.reboot-severs-open-session-rolls-back-uncommitted-keeps-committed|_rows#8|UndefinedTable|cur = conn.execute(sql, params)
Issue-Key: rds.read-replica-replication-query-and-promote|_sql_rows#15|UndefinedTable|cur.execute(statement)
Issue-Key: rds.e2e-parameter-change-affects-runtime-behavior|run#27|AssertionError|assert "statement timeout" in error_text.lower(), (
Issue-Key: rds.pg-dialect-numeric-timestamptz-jsonb-arrays-returning-recursive-cte|_rows#7|SystemError|cur = conn.execute(sql, params)
Issue-Key: rds.iam-db-auth-token-connects-and-maps-to-the-granted-role|_sql_rows#16|SystemError|cur.execute(statement)
Issue-Key: rds.iam-db-auth-token-minted-for-another-user-is-refused|_sql_rows#20|SystemError|cur.execute(statement, params)
Issue-Key: rds.copy-from-stdin-bulk-load-rejects-a-malformed-row|_copy#12|SystemError|with cur.copy(statement) as copy:
Issue-Key: rds.repeatable-read-transaction-sees-a-stable-snapshot|_sql_rows#18|SystemError|cur.execute(statement)
Issue-Key: rds.mysql-auto-increment-utf8mb4-and-strict-overflow-rejected|_rows#5|OperationalError|cur.execute(sql, params)
Issue-Key: rds.mysql-master-user-lacks-super-set-global-rejected|_rows#5|ProgrammingError|cur.execute(sql, params)
Issue-Key: rds.mysql-require-secure-transport-plaintext-login-rejected|_rows#13|OperationalError|cur.execute(sql, params)
Issue-Key: rds.mysql-wrong-master-password-login-rejected|run#10|AssertionError|assert _rows(conn, "SELECT CURRENT_USER()") == [(f"{_MASTER}@%",)], (
Issue-Key: rds.delete-instance-with-deletion-protection-rejected-rows-still-queryable|run#14|AssertionError|assert (code, status) == (_ERR_PROTECTED, 400), (
Issue-Key: rds.modify-master-password-below-policy-rejected-live-credential-unaffected|_error_code#16|AssertionError|raise AssertionError(
Issue-Key: rds.e2e-point-in-time-restore-recovers-data|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Key: rds.point-in-time-restore-data-query-round-trip|_poll#17|Timeout|time.sleep(interval_s)
Issue-Key: rds.snaprestore-restore-over-live-instance-rejected|run#26|AssertionError|assert code == "DBInstanceAlreadyExists", (
Issue-Commit: e32ddfa48df4fe30ed3566089db5dd37ef687284

# rds: 10 net-new causes — the engine data plane is not PostgreSQL/MySQL (no catalogs, GUCs, server functions or dialect), the published port is not the requested one, and four control-plane refusals/flows diverge

> _Note: this issue was identified against the code base at commit `e32ddfa48df4fe30ed3566089db5dd37ef687284`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstance

## Description
This issue bundles 10 net-new root causes in the rds service, all grounded in failing behavioural checks that the suite did NOT mark ALREADY FILED or OWNED. A session now DOES open on the published endpoint, so these are distinct from the open issue 20261104T053605Z-rds-the-published-db-instance-cluster-endpoint-serves.md ("serves no PostgreSQL/MySQL wire protocol"): the wire protocol answers, but what answers behind it is not the engine RDS provisions. A fixer must close all TEN causes below before submitting.

=== Cause 1 of 10: Endpoint.Port is the internal listener's port, not the port the request named (nor the engine default) ===
AWS: Smithy: CreateDBInstanceMessage$Port / ModifyDBInstanceMessage$DBPortNumber ("The port number on which the database accepts connections"); when Port is omitted RDS applies the engine default — 5432 for postgres/aurora-postgresql, 3306 for mysql/mariadb/aurora-mysql — and DescribeDBInstances publishes that value verbatim as DBInstance$Endpoint$Port. A client that asks for 5439 is told 5439.
Emulator: Endpoint.Port is whatever ephemeral port the internal engine listener was bound to. Checks that only assert the ENGINE DEFAULT (never a recording's literal value) fail: postgres instances report ports in the 36000–44000 range instead of 5432, mysql instances instead of 3306, and an instance created with Port=5439 reports a different port. The requested/default value is never published.
Why: every client that pins a connection string, a CloudFormation output or a security-group rule to the port it asked for (or to the documented engine default) connects to the wrong port; the published endpoint is not the endpoint the API contract describes.
Fix: the DB-instance endpoint publisher and the engine-listener binding in the rds crate's state/data-plane layer — the listener must bind the requested (or engine-default) port and the DescribeDBInstances serializer must publish that stored value, not the bound-listener value.
Related open issue (does not carry these check ids): 20261105T090000Z-rds-a-db-instance-publishes-the-listeners-port-not-the-requested-port.md.
Traces: rds.postgres-custom-port-is-the-published-and-listening-port, rds.deploy-db-instance-cfn-behaves, rds.engine-dialect-discriminator-query, rds.deep-rds-1-rds-engine-truth-mysql-constraint-lock-and-charset-violations-rejected, rds.deep-rds-sql-truth-iam-token-past-expiry-rejected, rds.deep-rds-sql-truth-mysql-replica-write-rejected-read-only.

=== Cause 2 of 10: the PostgreSQL endpoint exposes no server-configuration (GUC) surface ===
AWS: recorded behavior only (the parameter surface is a PostgreSQL contract, not a Smithy one). A PostgreSQL DB instance answers `SHOW <parameter>`, `SET <parameter> = ...`, `SELECT current_setting('<parameter>')` and `SELECT setting FROM pg_settings WHERE name = $1` for every parameter in the instance's DB parameter group (log_statement, statement_timeout, max_connections, default_transaction_isolation ...), and the value reported is the one the applied parameter group holds.
Emulator: those statements are refused by the backing store, which is not PostgreSQL: `unrecognized configuration parameter "log_statement"`, `no such function: current_setting in SELECT current_setting('max_connections')`, `no such table: pg_settings` (surfaced to clients as psycopg UndefinedTable). So no DB-parameter-group change can be observed on the engine at all.
Why: a client cannot read or set any server parameter, and nothing links ModifyDBParameterGroup/ApplyImmediately/pending-reboot semantics to engine behaviour — the whole parameter-group feature is unverifiable and silently inert.
Fix: the engine data plane's PostgreSQL emulation (GUC table + SHOW/SET/current_setting/pg_settings), wired to the instance's effective parameter group.
Traces: rds.cloudwatch-logs-export-delivers-only-while-enabled, rds.log-file-portion-contains-the-statement-the-engine-executed, rds.max-connections-cap-rejects-the-overflow-session, rds.paramgroup-out-of-range-value-rejected-engine-setting-unchanged, rds.serializable-write-skew-rejected-repeatable-read-permits, rds.static-parameter-pending-reboot-query-effect.

=== Cause 3 of 10: the PostgreSQL endpoint has no system catalogs ===
AWS: recorded behavior only. A PostgreSQL instance serves pg_database, pg_roles and pg_stat_activity; a session can read the database list, the role list and the live-session list, and `current_user`/`current_database()` agree with the instance's MasterUsername and DBName.
Emulator: `no such table: pg_database`, `no such table: pg_roles`, `no such table: pg_stat_activity` (psycopg UndefinedTable).
Why: clients and tooling that identify the session, enumerate databases/roles, or count connections get an error instead of a result set; the login-rejection semantics that depend on an unknown database being absent from pg_database cannot be exercised.
Fix: the engine data plane's PostgreSQL catalog emulation, populated from the instance's own state (DBName, MasterUsername, open sessions).
Traces: rds.postgres-session-identity-matches-master-username-and-db-name, rds.postgres-unknown-database-login-rejected, rds.pg-master-user-not-superuser-alter-system-rejected, rds.postgres-default-parameter-group-plaintext-login-rejected.

=== Cause 4 of 10: the PostgreSQL endpoint has no server functions ===
AWS: recorded behavior only. pg_postmaster_start_time(), pg_is_in_recovery() and pg_sleep(n) are all served by a PostgreSQL DB instance; pg_is_in_recovery() is how a read replica is distinguished from a writer, pg_postmaster_start_time() is how a reboot is observed, pg_sleep(n) is how a statement_timeout cancellation is observed.
Emulator: `no such function: pg_postmaster_start_time`, `no such function: pg_is_in_recovery`, `no such function: pg_sleep` (psycopg UndefinedTable). The statement_timeout check even reports the cancellation error as the missing-function error rather than a cancelled statement.
Why: reboot, failover, replica-role and statement-timeout behaviour are all unobservable from the engine, so a client cannot tell a writer from a replica or a restart from no restart.
Fix: the engine data plane's PostgreSQL function surface, backed by the instance lifecycle state the control plane already keeps (start time, replica role).
Traces: rds.deferred-class-modification-stays-pending-while-instance-serves-sql, rds.failover-force-failover-rejected-on-a-single-az-instance-moves-no-write-role, rds.reboot-severs-open-session-rolls-back-uncommitted-keeps-committed, rds.read-replica-replication-query-and-promote, rds.e2e-parameter-change-affects-runtime-behavior.

=== Cause 5 of 10: the PostgreSQL endpoint's SQL parser is not the PostgreSQL dialect ===
AWS: recorded behavior only. PostgreSQL accepts the `::` cast operator and numeric(p,s) casts, `CREATE ROLE "<name>" WITH LOGIN`, `COPY <table> FROM STDIN WITH (FORMAT text)` and `BEGIN ISOLATION LEVEL REPEATABLE READ`; a malformed COPY row is refused with PostgreSQL's own data-error, not a syntax error on the COPY statement itself.
Emulator: the parser refuses all of them with a SQLite-shaped syntax error reported as psycopg SystemError — `unrecognized token: ":" in SELECT 0.1::numeric ...`, `near "ROLE": syntax error in CREATE ROLE "..." WITH LOGIN`, `near "COPY": syntax error in COPY <t> FROM STDIN WITH (FORMAT text)`, `near "ISOLATION": syntax error in BEGIN ISOLATION LEVEL REPEATABLE READ`.
Why: ordinary PostgreSQL application SQL, bulk loading and explicit isolation levels fail with syntax errors against an endpoint that announces itself as PostgreSQL; the error class (syntax error) is also wrong for the data cases.
Fix: the engine data plane's PostgreSQL statement parser/executor (casts, role DDL, COPY FROM STDIN, transaction-isolation syntax).
Traces: rds.pg-dialect-numeric-timestamptz-jsonb-arrays-returning-recursive-cte, rds.iam-db-auth-token-connects-and-maps-to-the-granted-role, rds.iam-db-auth-token-minted-for-another-user-is-refused, rds.copy-from-stdin-bulk-load-rejects-a-malformed-row, rds.repeatable-read-transaction-sees-a-stable-snapshot.

=== Cause 6 of 10: the MySQL endpoint's SQL dialect and session identity diverge ===
AWS: recorded behavior only. A MySQL DB instance accepts table options (`... ENGINE=InnoDB DEFAULT CHARSET=utf8mb4`), serves `SHOW GRANTS FOR '<user>'@'%'`, serves system variables as `SELECT @@session.require_secure_transport` / `@@global....`, and `SELECT CURRENT_USER()` returns `<user>@<host-pattern>` (for the RDS master user, `<master>@%`).
Emulator: table options are a syntax error — `(1105, 'near "=": syntax error in CREATE TABLE ... ENGINE=InnoDB DEFAULT CHARSET=utf8mb4')`; `SHOW GRANTS FOR` is refused with `(1064, "this emulator's MySQL endpoint does not serve SHOW GRANTS FOR")`, which is emulator prose and not a MySQL error; `@@session.<var>` is rewritten to `SELECT NULL.<var>` and then refused `(1105, 'near ".": syntax error ...')`; `SELECT CURRENT_USER()` returns the bare user name (`sim2real`) with no `@host` part.
Why: standard MySQL DDL fails, privilege and TLS-enforcement introspection is impossible, and the session identity a client reads back is not a MySQL account identity, so authorization logic keyed on `user@host` misbehaves.
Fix: the engine data plane's MySQL emulation — table-option parsing, SHOW GRANTS, system-variable reads, and CURRENT_USER()/USER() rendering as `user@host`.
Traces: rds.mysql-auto-increment-utf8mb4-and-strict-overflow-rejected, rds.mysql-master-user-lacks-super-set-global-rejected, rds.mysql-require-secure-transport-plaintext-login-rejected, rds.mysql-wrong-master-password-login-rejected.

=== Cause 7 of 10: DeleteDBInstance on a deletion-protected instance is classified as a state fault ===
AWS: deleting an instance with DeletionProtection=true is a parameter-combination refusal, HTTP 400, error code InvalidParameterCombination, message "Cannot delete protected DB Instance <identifier>, please disable deletion protection and try again." (fixed text; the identifier is the DBInstanceIdentifier from the request). Smithy: DeleteDBInstance does not model InvalidDBInstanceStateFault for this case as the protection gate — InvalidDBInstanceState (400) is the lifecycle fault for an instance that is already creating/modifying/deleting.
Emulator: the call is refused 400 with code InvalidDBInstanceState, so a protected instance is indistinguishable from one in a transient state.
Why: a client that retries on a state fault (expecting the state to pass) loops forever instead of surfacing the configuration error it must fix by disabling DeletionProtection.
Fix: the DeleteDBInstance handler's validation ordering/classification in the rds crate.
Related open issue (does not carry this check id): 20261104T053605Z-rds-9-net-new-causes-refusals-are-classified.md.
Traces: rds.delete-instance-with-deletion-protection-rejected-rows-still-queryable.

=== Cause 8 of 10: ModifyDBInstance accepts a MasterUserPassword below the engine's password policy ===
AWS: Smithy: ModifyDBInstanceMessage$MasterUserPassword; RDS enforces the engine's master-password length policy (8–128 printable characters for PostgreSQL/MySQL) and refuses a shorter value with HTTP 400 InvalidParameterValue, message "The parameter MasterUserPassword is not a valid password. Only printable ASCII characters besides '/', '@', '\"', ' ' may be used." / the length sentence for a too-short value — the member name and the limit are the fixed parts, the offending value is never echoed.
Emulator: the call returns 200 with DBInstance.DBInstanceStatus="modifying" and no error at all.
Why: a client that relies on RDS to reject a weak admin password believes the weak password was installed (or, worse, that its rotation succeeded) when nothing was validated; the negative path of every credential-policy test disappears.
Fix: ModifyDBInstance's input validation for MasterUserPassword in the rds crate.
Traces: rds.modify-master-password-below-policy-rejected-live-credential-unaffected.

=== Cause 9 of 10: DescribeDBInstances never reports LatestRestorableTime, so point-in-time restore can never be exercised ===
AWS: Smithy: DBInstance$LatestRestorableTime — for an instance with BackupRetentionPeriod >= 1, RDS reports a LatestRestorableTime that advances (trailing real time by roughly five minutes) and RestoreDBInstanceToPointInTime restores to any instant inside [LatestRestorableTime - retention, LatestRestorableTime]; a RestoreTime outside that window is refused 400 InvalidParameterValue naming the window bound, and PointInTimeRestoreNotEnabled (400) is the refusal when continuous backups are off.
Emulator: both PITR checks poll DescribeDBInstances for LatestRestorableTime on an instance created with backup retention enabled and never see one; the checks are cut off at the harness cap ("Check timed out after 60.0s") with no value ever published. Hypothesis: no continuous-backup window is tracked for a DB instance, so the member is simply never serialized.
Why: a client cannot discover the recovery window, so point-in-time recovery — including the window-bound refusals — is unreachable, and a backup/restore runbook silently never progresses.
Fix: the DB-instance state's continuous-backup window (set and advanced while BackupRetentionPeriod >= 1) and the DescribeDBInstances serializer that must publish LatestRestorableTime from it.
Traces: rds.e2e-point-in-time-restore-recovers-data, rds.point-in-time-restore-data-query-round-trip.

=== Cause 10 of 10: restoring a snapshot onto the identifier of a LIVE instance is not refused with DBInstanceAlreadyExists ===
AWS: Smithy: RestoreDBInstanceFromDBSnapshot models DBInstanceAlreadyExistsFault, awsQueryError code DBInstanceAlreadyExists, HTTP 400, message "DB instance already exists" — the identifier-collision check precedes any restore work, so naming an existing instance is refused before the snapshot is read.
Emulator: the call answers code NotImplemented (the operation is a scaffold stub), so the collision guard never runs and the refusal class a client branches on is unreachable. The stub itself is covered by the auto-filed "Implement rds RestoreDBInstanceFromDBSnapshot" issue; what is filed HERE is the behaviour the implementation must carry — the DBInstanceAlreadyExists precedence on a live target identifier — because this check is graded on the error code, not on the stub.
Why: a client that uses the collision refusal to detect "already restored" sees NotImplemented and cannot distinguish an unsupported operation from a name clash.
Fix: RestoreDBInstanceFromDBSnapshot's handler and its identifier-collision validation in the rds crate.
Traces: rds.snaprestore-restore-over-live-instance-rejected.

## Evidence
Cause 1: DescribeDBInstances Endpoint.Port for a postgres instance = 41235/36233/43807 (ephemeral) where 5432 (or the requested 5439) is required; for mysql = 40383/42149/32949 where 3306 is required.
Cause 2: psycopg.errors.UndefinedTable: unrecognized configuration parameter "log_statement"; no such function: current_setting in SELECT current_setting('max_connections'); no such table: pg_settings.
Cause 3: psycopg.errors.UndefinedTable: no such table: pg_database / pg_roles / pg_stat_activity.
Cause 4: psycopg.errors.UndefinedTable: no such function: pg_postmaster_start_time in SELECT pg_postmaster_start_time(); no such function: pg_is_in_recovery; 'no such function: pg_sleep in SELECT pg_sleep(2)' returned where a statement_timeout cancellation was required.
Cause 5: psycopg.errors.SystemError: unrecognized token: ":" in SELECT 0.1::numeric + 0.2::numeric ...; near "ROLE": syntax error in CREATE ROLE "..." WITH LOGIN; near "COPY": syntax error in COPY <table> FROM STDIN WITH (FORMAT text); near "ISOLATION": syntax error in BEGIN ISOLATION LEVEL REPEATABLE READ.
Cause 6: pymysql.err.OperationalError (1105) 'near "=": syntax error in CREATE TABLE ... ENGINE=InnoDB DEFAULT CHARSET=utf8mb4'; pymysql.err.ProgrammingError (1064) "this emulator's MySQL endpoint does not serve SHOW GRANTS FOR"; (1105) 'near ".": syntax error in SELECT NULL.require_secure_transport'; SELECT CURRENT_USER() returned [('sim2real',)] where [('sim2real@%',)] is required.
Cause 7: DeleteDBInstance on a DeletionProtection=true instance answered ('InvalidDBInstanceState', 400); expected ('InvalidParameterCombination', 400).
Cause 8: ModifyDBInstance with a below-policy MasterUserPassword returned HTTP 200 with DBInstance.DBInstanceStatus='modifying' instead of a 400 InvalidParameterValue.
Cause 9: DescribeDBInstances never returned LatestRestorableTime for an instance with BackupRetentionPeriod>=1; both PITR checks ended with "Check timed out after 60.0s" before any restore was attempted.
Cause 10: RestoreDBInstanceFromDBSnapshot targeting the identifier of a live instance answered code 'NotImplemented'; expected 400 DBInstanceAlreadyExists.

## Steps to Reproduce
Cause 1: CreateDBInstance(Engine=postgres, Port=5439) and a second with Port omitted; poll to available; read DescribeDBInstances DBInstance.Endpoint.Port — it is neither 5439 nor 5432. Repeat with Engine=mysql and expect 3306.
Causes 2-5: create a postgres instance, open a psycopg session on the published endpoint, then run: SHOW log_statement; SELECT current_setting('max_connections'); SELECT setting FROM pg_settings WHERE name='max_connections'; SELECT * FROM pg_database; SELECT * FROM pg_roles; SELECT * FROM pg_stat_activity; SELECT pg_postmaster_start_time(); SELECT pg_is_in_recovery(); SELECT pg_sleep(2); SELECT 0.1::numeric; CREATE ROLE "r1" WITH LOGIN; COPY t FROM STDIN WITH (FORMAT text); BEGIN ISOLATION LEVEL REPEATABLE READ.
Cause 6: create a mysql instance, open a pymysql session, then run: CREATE TABLE t (id INT AUTO_INCREMENT PRIMARY KEY) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4; SHOW GRANTS FOR 'master'@'%'; SELECT @@session.require_secure_transport; SELECT CURRENT_USER().
Cause 7: CreateDBInstance(DeletionProtection=true); wait available; DeleteDBInstance(SkipFinalSnapshot=true) and read the error code.
Cause 8: CreateDBInstance then ModifyDBInstance(MasterUserPassword='short', ApplyImmediately=true) and read the status/code.
Cause 9: CreateDBInstance(BackupRetentionPeriod=1); wait available; poll DescribeDBInstances for LatestRestorableTime.
Cause 10: CreateDBInstance A, CreateDBSnapshot of A, then RestoreDBInstanceFromDBSnapshot(DBInstanceIdentifier=A, DBSnapshotIdentifier=<snap>) and read the error code.
