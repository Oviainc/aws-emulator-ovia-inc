Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.engine-dialect-discriminator-query rds.aurora-mysql-backtrack-removes-rows-after-time-keeps-before rds.aurora-mysql-clone-copy-on-write-sees-source-rows-diverges-after rds.deep-rds-1-rds-engine-truth-mysql-constraint-lock-and-charset-violations-rejected rds.binlog-retention-hours-configuration-round-trip rds.mysql-iam-db-auth-token-binding-and-validity-enforced rds.mysql-unknown-database-login-rejected rds.rds-kill-ends-session-with-lost-connection-error rds.blue-green-deployment-mysql-green-read-only-and-switchover-renames-blue rds.deep-rds-sql-truth-mysql-replica-write-rejected-read-only rds.mariadb-server-version-matches-the-described-engine-version rds.mariadb-require-secure-transport-plaintext-login-rejected rds.mariadb-duplicate-primary-key-and-utf8mb4-round-trip rds.deep-rds-1-rds-engine-truth-aurora-postgres-constraint-and-lock-violations-rejected rds.postgres-admin-user-lacks-pg-read-server-files-role rds.postgres-create-extension-restricted-by-allowed-extensions rds.aurora-clone-copy-on-write-isolation rds.postgres-idle-in-transaction-session-timeout-terminates-session rds.mariadb-admin-user-lacks-super-set-global-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.engine-dialect-discriminator-query|run#12|AssertionError|assert exc is not None, (
Issue-Key: rds.aurora-mysql-backtrack-removes-rows-after-time-keeps-before|_sql_rows#18|ProgrammingError|cur.execute(statement)
Issue-Key: rds.aurora-mysql-clone-copy-on-write-sees-source-rows-diverges-after|_sql_rows#14|ProgrammingError|cur.execute(statement)
Issue-Key: rds.deep-rds-1-rds-engine-truth-mysql-constraint-lock-and-charset-violations-rejected|_exec#6|ProgrammingError|return cur.execute(statement, params)
Issue-Key: rds.binlog-retention-hours-configuration-round-trip|_rows#5|ProgrammingError|cur.execute(sql, params)
Issue-Key: rds.mysql-iam-db-auth-token-binding-and-validity-enforced|run#11|ProgrammingError|admin.query(
Issue-Key: rds.mysql-unknown-database-login-rejected|_rows#9|ProgrammingError|cur.execute(sql, params)
Issue-Key: rds.rds-kill-ends-session-with-lost-connection-error|_rows#7|ProgrammingError|cur.execute(sql, params)
Issue-Key: rds.blue-green-deployment-mysql-green-read-only-and-switchover-renames-blue|run#31|AssertionError|assert green_ro == [(1,)], f"green must report @@read_only 1 before switchover; got {green_ro!r}"
Issue-Key: rds.deep-rds-sql-truth-mysql-replica-write-rejected-read-only|run#15|AssertionError|assert ro == [(0,)], f"the source must be writable (@@read_only 0); got {ro!r}"
Issue-Key: rds.mariadb-server-version-matches-the-described-engine-version|run#9|AssertionError|assert "MariaDB" in value, f"{label} must contain 'MariaDB'; got {value!r}"
Issue-Key: rds.mariadb-require-secure-transport-plaintext-login-rejected|run#13|AssertionError|assert setting == [(1,)], f"require_secure_transport must read ON; got {setting!r}"
Issue-Key: rds.mariadb-duplicate-primary-key-and-utf8mb4-round-trip|_duplicate_key_beat#6|AssertionError|assert refused == (1062, "Duplicate entry '1' for key 'PRIMARY'"), (
Issue-Key: rds.deep-rds-1-rds-engine-truth-aurora-postgres-constraint-and-lock-violations-rejected|_unique_beat#6|AssertionError|assert refused == ("23505", "s2r_users_email_uq"), (
Issue-Key: rds.postgres-admin-user-lacks-pg-read-server-files-role|_rows#13|SyntaxError|cur = conn.execute(sql, params)
Issue-Key: rds.postgres-create-extension-restricted-by-allowed-extensions|_rows#21|UndefinedTable|cur = conn.execute(sql, params)
Issue-Key: rds.aurora-clone-copy-on-write-isolation|_sql_rows#17|SyntaxError|cur.execute(statement, params)
Issue-Key: rds.postgres-idle-in-transaction-session-timeout-terminates-session|_rows#21|InsufficientPrivilege|cur = conn.execute(sql, params)
Issue-Key: rds.mariadb-admin-user-lacks-super-set-global-rejected|run#5|AssertionError|assert (code, msg) == (
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# rds: 10 net-new causes — the SQL endpoints are a generic shim, not MySQL/MariaDB/PostgreSQL: dialect, catalogs, server variables, constraint errors and privilege boundaries all diverge

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstance

## Description
10 net-new root causes in the emulated database engines reached through the published RDS endpoints. Related open issues describe other aspects of the same endpoints (20261106T031005Z-rds-7-net-new-causes-endpoint-port-publishes.md, 20261109T020152Z-rds-3-net-new-causes-an-aurora-postgresql.md); none of them carries these replay keys.

Cause 1 of 10: a postgres endpoint accepts MySQL-only syntax, so it is not speaking the PostgreSQL dialect.
AWS: a postgres/aurora-postgresql endpoint refuses 'SELECT @@version_comment' with a syntax error (SQLSTATE 42601); only MySQL-family engines accept the @@variable form. Recorded behavior only.
Emulator: the postgres endpoint ACCEPTS it and returns rows, so engine identity cannot be discriminated at all.
Why: a client that branches on dialect (ORMs, migration tools, any 'which engine am I on' probe) takes the wrong branch, and no engine-specific behaviour below can be trusted.
Fix: the SQL front-end's dialect dispatch for the postgres wire endpoint.
Traces: rds.engine-dialect-discriminator-query.

Cause 2 of 10: MySQL date/time functions are unimplemented - UTC_TIMESTAMP(6) is a syntax error.
AWS: MySQL/Aurora MySQL evaluate UTC_TIMESTAMP(6) and return a microsecond-precision UTC timestamp.
Emulator: the statement is refused with error 1064 'You have an error in your SQL syntax ... near 'UTC_TIMESTAMP(6)''.
Why: any client timestamping rows server-side (and every backtrack/clone test that needs a server clock) fails with a syntax error on valid MySQL.
Fix: the MySQL dialect's function set in the SQL engine.
Traces: rds.aurora-mysql-backtrack-removes-rows-after-time-keeps-before, rds.aurora-mysql-clone-copy-on-write-sees-source-rows-diverges-after.

Cause 3 of 10: MySQL CREATE TABLE cannot declare an inline named UNIQUE KEY.
AWS: 'CREATE TABLE ... (..., UNIQUE KEY s2r_users_email_uq (email))' is valid MySQL DDL and creates a named unique index.
Emulator: refused 1064 '... near 'KEY s2r_users_email_uq (email))''.
Why: ordinary MySQL schemas will not deploy; a client cannot create the named constraints its later error handling keys on.
Fix: the MySQL DDL grammar in the SQL engine (table-constraint clause).
Traces: rds.deep-rds-1-rds-engine-truth-mysql-constraint-lock-and-charset-violations-rejected.

Cause 4 of 10: RDS's own MySQL administrative statements are unimplemented.
AWS: 'CALL mysql.rds_show_configuration' returns the instance's binlog retention configuration, and 'CREATE USER '<user>'@'%' IDENTIFIED WITH AWSAuthenticationPlugin AS 'RDS'' creates an IAM-authenticated database user. Both are documented RDS MySQL extensions.
Emulator: both are refused 1064 as syntax errors (near 'CALL rds_show_configuration' and near 'IDENTIFIED WITH AWSAuthenticationPlugi').
Why: binlog retention cannot be configured or read, and no IAM database user can be created, so IAM DB authentication cannot be set up through the engine at all.
Fix: the MySQL dialect's RDS stored-procedure and CREATE USER ... IDENTIFIED WITH support in the SQL engine.
Traces: rds.binlog-retention-hours-configuration-round-trip, rds.mysql-iam-db-auth-token-binding-and-validity-enforced.

Cause 5 of 10: the MySQL system catalogs are missing.
AWS: information_schema.SCHEMATA lists the databases on the instance and information_schema.PROCESSLIST lists live sessions; both are always present.
Emulator: queries against them fail with 1146 'no such table: schemata' and 1146 'no such table: PROCESSLIST' - and the SQLite-style message text is itself not MySQL's ('Table '<db>.<table>' doesn't exist').
Why: a client cannot enumerate databases or sessions, cannot find a connection id to kill, and receives a 1146 message in a shape no MySQL client parser expects.
Fix: the MySQL catalog views exposed by the SQL engine, and the 1146 message template.
Traces: rds.mysql-unknown-database-login-rejected, rds.rds-kill-ends-session-with-lost-connection-error.

Cause 6 of 10: server variables read back with wrong types or no value.
AWS: 'SELECT @@read_only' returns 1 on a read-only member and 0 on a writer; '@@version' on MariaDB contains 'MariaDB'; 'SHOW VARIABLES'/'SELECT @@require_secure_transport' returns the integer 1 when it is ON.
Emulator: @@read_only returns NULL (both for a blue/green green member and for a writable source), @@version is '11.4.4' with no 'MariaDB' token, and require_secure_transport reads back as the string '1' rather than the integer 1.
Why: a client cannot tell a writer from a reader (the standard replica probe), cannot detect MariaDB vs MySQL from the server banner, and typed drivers mis-decode a string where an integer is modeled.
Fix: the SQL engine's session/global variable table and its value typing.
Traces: rds.blue-green-deployment-mysql-green-read-only-and-switchover-renames-blue, rds.deep-rds-sql-truth-mysql-replica-write-rejected-read-only, rds.mariadb-server-version-matches-the-described-engine-version, rds.mariadb-require-secure-transport-plaintext-login-rejected.

Cause 7 of 10: constraint-violation errors are SQLite-shaped, not engine-shaped.
AWS: MariaDB/MySQL refuse a duplicate primary key with error 1062 and the message "Duplicate entry '<value>' for key '<index name>'" - fixed text with the offending value and index name substituted ('PRIMARY' for the primary key). PostgreSQL refuses a duplicate on a named UNIQUE constraint with SQLSTATE 23505 and reports the constraint name in the error's constraint field.
Emulator: MariaDB gives 1062 with 'UNIQUE constraint failed: s2r_accounts.id' (SQLite's wording, no 'PRIMARY'), and PostgreSQL gives 23505 with constraint = None.
Why: a client that parses the message or reads the constraint field to decide which unique rule it violated - the standard upsert/conflict path - learns nothing.
Fix: the SQL engine's error mapping from the storage layer to each engine's error code/message/fields.
Traces: rds.mariadb-duplicate-primary-key-and-utf8mb4-round-trip, rds.deep-rds-1-rds-engine-truth-aurora-postgres-constraint-and-lock-violations-rejected.

Cause 8 of 10: PostgreSQL catalogs and functions are missing.
AWS: pg_has_role(...) is a built-in function, pg_available_extensions is a system view, and 'now' is usable where PostgreSQL accepts it (now(), CURRENT_TIMESTAMP).
Emulator: pg_has_role is a syntax error, pg_available_extensions is 'no such table', and a statement using now fails with 'syntax error at or near "now"'.
Why: role/privilege introspection and extension discovery - the documented way to check what an RDS PostgreSQL admin may do and which extensions are allowed - are impossible, and ordinary timestamp SQL fails.
Fix: the PostgreSQL dialect's catalog views and built-in function set in the SQL engine.
Traces: rds.postgres-admin-user-lacks-pg-read-server-files-role, rds.postgres-create-extension-restricted-by-allowed-extensions, rds.aurora-clone-copy-on-write-isolation.

Cause 9 of 10: the PostgreSQL endpoint refuses bound parameters in the binary format.
AWS: a PostgreSQL endpoint accepts the extended query protocol with binary-format parameters; that is what every modern driver sends by default.
Emulator: the statement is refused with SQLSTATE 42501 (InsufficientPrivilege) and the emulator-authored message 'this endpoint reads parameters in the text format only' - a privilege error class for what is a protocol limitation, and a message AWS never sends.
Why: a driver that binds parameters in binary gets a permission error it will surface as an authorization problem, sending the client after the wrong cause.
Fix: the PostgreSQL wire protocol implementation's Bind handling in the SQL front-end.
Traces: rds.postgres-idle-in-transaction-session-timeout-terminates-session.

Cause 10 of 10: the master user's privilege boundary is not enforced on MariaDB.
AWS: the RDS master user is not a superuser; SET GLOBAL is refused with MariaDB error 1227 and the message 'Access denied; you need (at least one of) the SUPER or SYSTEM_VARIABLES_ADMIN privilege(s) for this operation' - fixed text for this class.
Emulator: SET GLOBAL succeeds for the master user, so the refusal never happens.
Why: a client that relies on RDS's managed-instance boundary (or tests that its code degrades when SET GLOBAL is denied) sees a permissive server AWS never gives, and parameter changes that must go through a parameter group appear to work at session level.
Fix: the SQL engine's privilege model for the master user, in its SET GLOBAL / variable-write path.
Traces: rds.mariadb-admin-user-lacks-super-set-global-rejected.

## Evidence
Cause 1: AssertionError: the postgres endpoint ACCEPTED mysql-only syntax (SELECT @@version_comment).
Cause 2: pymysql ProgrammingError (1064, "... near 'UTC_TIMESTAMP(6)' at line 1").
Cause 3: (1064, "... near 'KEY s2r_users_email_uq (email))' at line 1").
Cause 4: (1064, "... near 'CALL rds_show_configuration' at line 1"); (1064, "... near ''sim2real@127.0.0.1' 'sim2real_iam_a'@'%' IDENTIFIED WITH AWSAuthenticationPlugi' at line 1").
Cause 5: (1146, 'no such table: schemata'); (1146, 'no such table: PROCESSLIST').
Cause 6: '@@read_only ... got [(None,)]' (twice); '@@version must contain MariaDB; got '11.4.4''; 'require_secure_transport must read ON; got [('1',)]'.
Cause 7: 'a duplicate PRIMARY KEY must be refused with MariaDB error 1062 naming PRIMARY; got (1062, 'UNIQUE constraint failed: s2r_accounts.id')'; 'must be refused with SQLSTATE 23505 naming s2r_users_email_uq; got ('23505', None)'.
Cause 8: psycopg SyntaxError: syntax error at or near "pg_has_role"; UndefinedTable: no such table: pg_available_extensions; SyntaxError: syntax error at or near "now".
Cause 9: psycopg InsufficientPrivilege: this endpoint reads parameters in the text format only.
Cause 10: 'SET GLOBAL must be refused for the admin user: (1227, 'Access denied; you need (at least one of) the SUPER or SYSTEM_VARIABLES_ADMIN privilege(s) for this operation')' - the refusal never came.

## Steps to Reproduce
1. Create db instances/clusters for postgres, mysql, aurora-mysql and mariadb and connect to each published endpoint.
2. Cause 1: run 'SELECT @@version_comment' on the postgres endpoint.
3. Cause 2: run 'SELECT UTC_TIMESTAMP(6)' on an aurora-mysql endpoint.
4. Cause 3: CREATE TABLE with an inline 'UNIQUE KEY <name> (<col>)'.
5. Cause 4: 'CALL mysql.rds_show_configuration'; 'CREATE USER ... IDENTIFIED WITH AWSAuthenticationPlugin AS 'RDS''.
6. Cause 5: query information_schema.SCHEMATA and information_schema.PROCESSLIST.
7. Cause 6: 'SELECT @@read_only', 'SELECT @@version', 'SELECT @@require_secure_transport'.
8. Cause 7: insert a duplicate primary key on MariaDB and a duplicate on a named UNIQUE constraint on PostgreSQL.
9. Cause 8: 'SELECT pg_has_role(...)', 'SELECT * FROM pg_available_extensions', a statement using now.
10. Cause 9: execute a parameterised statement with a binary-format bind.
11. Cause 10: 'SET GLOBAL ...' as the master user on MariaDB.
