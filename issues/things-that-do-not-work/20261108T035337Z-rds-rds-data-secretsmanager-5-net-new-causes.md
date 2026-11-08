Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__rds-data__secretsmanager.concurrent-data-api-and-native-increments-lose-no-update rds__rds-data__secretsmanager.data-api-insert-is-read-by-a-native-postgres-session rds__rds-data__secretsmanager.data-api-open-transaction-is-invisible-to-a-native-session-until-commit rds__rds-data__secretsmanager.mysql-repeatable-read-snapshot-ignores-a-later-commit-until-it-ends rds__rds-data__secretsmanager.data-api-and-native-mysql-sessions-read-each-others-writes rds__rds-data__secretsmanager.mysql-lock-wait-timeout-rejects-an-update-of-a-row-a-data-api-transaction-holds rds__rds-data__secretsmanager.data-api-row-lock-makes-a-native-nowait-lock-rejected-until-commit rds__rds-data__secretsmanager.native-insert-is-read-by-the-data-api-postgres rds__rds-data__secretsmanager.typed-values-agree-between-the-data-api-and-a-native-session rds__rds-data__secretsmanager.native-uncommitted-write-is-invisible-to-a-second-session-under-read-committed
Issue-Key: rds__rds-data__secretsmanager.concurrent-data-api-and-native-increments-lose-no-update|_native_loop#2|UndefinedTable|cur.execute(statement)
Issue-Key: rds__rds-data__secretsmanager.data-api-insert-is-read-by-a-native-postgres-session|_rows#2|UndefinedTable|cur.execute(statement, args)
Issue-Key: rds__rds-data__secretsmanager.data-api-open-transaction-is-invisible-to-a-native-session-until-commit|_rows#2|UndefinedTable|cur.execute(statement, args)
Issue-Key: rds__rds-data__secretsmanager|ExecuteStatement|DatabaseErrorException
Issue-Key: rds__rds-data__secretsmanager.data-api-row-lock-makes-a-native-nowait-lock-rejected-until-commit|_rows#2|SyntaxError|cur.execute(statement, args)
Issue-Key: rds__rds-data__secretsmanager.native-insert-is-read-by-the-data-api-postgres|_rows#2|InsufficientPrivilege|cur.execute(statement, args)
Issue-Key: rds__rds-data__secretsmanager.typed-values-agree-between-the-data-api-and-a-native-session|_rows#2|InsufficientPrivilege|cur.execute(statement, args)
Issue-Key: rds__rds-data__secretsmanager.native-uncommitted-write-is-invisible-to-a-second-session-under-read-committed|_grade#2|AssertionError|assert b_before == [(1, "old")], (
Issue-Commit: dd999dbcd4bb2e7cddb5a2ba7d5e91a60410d27b

# rds__rds-data__secretsmanager: 5 net-new causes — the Data API and the native engine endpoint are not the same database, MySQL DDL is refused, SELECT…FOR UPDATE and binary-format parameters are refused, and the native endpoint has no transaction isolation

> _Note: this issue was identified against the code base at commit `dd999dbcd4bb2e7cddb5a2ba7d5e91a60410d27b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Five distinct net-new root causes in the cross-surface (RDS Data API + native engine wire protocol) behaviour of one Aurora cluster. All of them are grounded in failing checks of this service group.

Cause 1 of 5: the RDS Data API and the cluster's native engine endpoint do not address the same database — data written on one surface is not visible on the other.
AWS: recorded behavior only (the Data API is a signed HTTP front door onto the SAME cluster; the model does not describe storage). AWS: a table created and rows inserted through rds-data ExecuteStatement against a cluster ARN are immediately readable by a native psycopg/MySQL session connected to that cluster's writer endpoint, and vice versa; a committed write on either surface is seen by the other.
Emulator: a table created through the Data API does not exist for the native session — psycopg raises UndefinedTable 'no such table: <table>' — and a table created natively does not exist for the Data API — ExecuteStatement answers DatabaseErrorException 'ERROR: relation "<table>" does not exist; SQLState: 42P01'. The two surfaces are backed by separate stores. (Note on that second message: the cluster in those checks is MySQL, yet the emulator answers with a PostgreSQL SQLSTATE/wording — the engine dialect of the refusal does not follow the cluster's engine either.)
Why: the Data API is documented as another door onto the same cluster. A client that bootstraps schema or seeds rows over the Data API and then connects natively (or the reverse — the common pattern for migrations, admin jobs and tests) sees an empty database, and every read-your-writes, visibility and locking guarantee between the two surfaces is lost.
Fix: the rds-data ExecuteStatement/BatchExecuteStatement handler and the RDS cluster's native-endpoint session state must resolve to ONE backing database per cluster, keyed by the cluster (resourceArn) rather than per-surface.
Traces: rds__rds-data__secretsmanager.concurrent-data-api-and-native-increments-lose-no-update; rds__rds-data__secretsmanager.data-api-insert-is-read-by-a-native-postgres-session; rds__rds-data__secretsmanager.data-api-open-transaction-is-invisible-to-a-native-session-until-commit; rds__rds-data__secretsmanager.mysql-repeatable-read-snapshot-ignores-a-later-commit-until-it-ends.

Cause 2 of 5: the SQL parser behind the Data API refuses MySQL table options, so ordinary Aurora MySQL DDL cannot be executed.
AWS: recorded behavior only. Aurora MySQL accepts `CREATE TABLE <t> (<cols>) ENGINE = InnoDB` through rds-data ExecuteStatement and returns a successful (empty) result; DatabaseErrorException is reserved for SQL the engine itself rejects, and its message is the engine's own error text.
Emulator: ExecuteStatement answers DatabaseErrorException with the message `near "=": syntax error in CREATE TABLE "<table>" (...) ENGINE = InnoDB at offset <n>` — a SQLite-style parser error. The fixed text is `near "<token>": syntax error in ` / ` at offset `; the variable parts are the offending token, the statement echoed back and the byte offset. No MySQL-engine error has that form.
Why: any Aurora MySQL client whose schema DDL names a storage engine (the default idiom emitted by mysqldump and most ORMs/migration tools) cannot create its schema at all, and the refusal is reported as a database error, so the client concludes its own SQL is invalid.
Fix: the SQL dialect/parser layer backing rds-data ExecuteStatement — it must accept MySQL DDL (table options such as ENGINE=) when the target cluster's engine is MySQL/Aurora MySQL. Open issue 20261106T233903Z-rds-data-9-net-new-root-causes-mysql.md records the same parser gap under the `rds-data` service token but does not carry these checks' replay keys.
Traces: rds__rds-data__secretsmanager.data-api-and-native-mysql-sessions-read-each-others-writes; rds__rds-data__secretsmanager.mysql-lock-wait-timeout-rejects-an-update-of-a-row-a-data-api-transaction-holds.

Cause 3 of 5: the native PostgreSQL endpoint refuses row-level locking clauses as a syntax error.
AWS: recorded behavior only. Aurora PostgreSQL accepts `SELECT … FOR UPDATE` and `SELECT … FOR UPDATE NOWAIT`; when the row is already locked by another transaction, NOWAIT fails with the engine's lock-not-available condition (SQLSTATE 55P03), not with a syntax error.
Emulator: the native session's `SELECT … FOR UPDATE [NOWAIT]` is rejected at parse time — psycopg raises SyntaxError 'syntax error at or near "FOR"'. The locking clause is not implemented at all.
Why: pessimistic locking is the standard way clients serialise concurrent updates. A client issuing SELECT…FOR UPDATE gets a syntax error, so correct code looks broken, and no lock-contention behaviour can be exercised.
Fix: the native PostgreSQL endpoint's SQL grammar/executor — parse and honour the FOR UPDATE / FOR SHARE locking clause and its NOWAIT / SKIP LOCKED options, and refuse a contended NOWAIT with the engine's lock-not-available error rather than a parse error.
Traces: rds__rds-data__secretsmanager.data-api-row-lock-makes-a-native-nowait-lock-rejected-until-commit.

Cause 4 of 5: the native PostgreSQL endpoint refuses binary-format parameters, and classifies the refusal as a privilege error.
AWS: recorded behavior only. Aurora PostgreSQL implements the extended query protocol and accepts parameters in both the text and the binary format (psycopg3's default for many Python types), so a parameterised execute of typed values succeeds.
Emulator: the endpoint answers an error psycopg maps to InsufficientPrivilege with the message 'this endpoint reads parameters in the text format only'. That is both a refusal AWS never issues and the wrong error class: a parameter-format problem is not an insufficient-privilege (42501) condition.
Why: every client library that sends binary parameters — the default for psycopg3, asyncpg and JDBC prepared statements with typed values — cannot execute a parameterised statement at all; and because the failure is surfaced as a privilege error, the client is pointed at its grants instead of at the format.
Fix: the native PostgreSQL endpoint's extended-query/Bind handling — decode binary-format parameter values (and binary result formats) per the wire protocol; if some format genuinely cannot be supported, it must not be reported under the insufficient-privilege class.
Traces: rds__rds-data__secretsmanager.native-insert-is-read-by-the-data-api-postgres; rds__rds-data__secretsmanager.typed-values-agree-between-the-data-api-and-a-native-session.

Cause 5 of 5: the native endpoint provides no transaction isolation — one session reads another session's uncommitted writes.
AWS: recorded behavior only. Aurora PostgreSQL's default READ COMMITTED isolation means session B, reading a table while session A holds an open transaction that has UPDATEd a row and INSERTed another, sees only the pre-transaction committed state; A's changes appear to B only after A commits.
Emulator: session B sees A's uncommitted UPDATE and uncommitted INSERT immediately — it reads the updated row value and the row A has not committed yet (the check observed two rows including A's uncommitted insert where only the single committed row must be visible).
Why: dirty reads break every client that relies on transactional atomicity: a reader can observe a half-applied multi-statement change, and a transaction that is later rolled back has already been consumed by another session.
Fix: the native endpoint's transaction/session state — per-session transaction buffers with at least READ COMMITTED visibility, so uncommitted changes are invisible to other sessions until COMMIT (and discarded on ROLLBACK).
Traces: rds__rds-data__secretsmanager.native-uncommitted-write-is-invisible-to-a-second-session-under-read-committed.

## Evidence
Cause 1 (cross-surface invisibility):
  concurrent-data-api-and-native-increments-lose-no-update — psycopg.errors.UndefinedTable: no such table: <counter table created via the Data API>
  data-api-insert-is-read-by-a-native-postgres-session — psycopg.errors.UndefinedTable: no such table: <table created via the Data API>
  data-api-open-transaction-is-invisible-to-a-native-session-until-commit — psycopg.errors.UndefinedTable: no such table: <table created via the Data API>
  mysql-repeatable-read-snapshot-ignores-a-later-commit-until-it-ends — DatabaseErrorException on ExecuteStatement: 'ERROR: relation "<table created natively>" does not exist; SQLState: 42P01' (and a PostgreSQL SQLSTATE for a MySQL cluster)
  Expected: the table/rows created on one surface are visible on the other.

Cause 2 (MySQL DDL refused):
  data-api-and-native-mysql-sessions-read-each-others-writes — DatabaseErrorException (ExecuteStatement): 'near "=": syntax error in CREATE TABLE "<table>" ("id" INT PRIMARY KEY, "label" VARCHAR(64) NOT NULL) ENGINE = InnoDB at offset <n>'
  mysql-lock-wait-timeout-rejects-an-update-of-a-row-a-data-api-transaction-holds — same refusal shape for its own CREATE TABLE … ENGINE = InnoDB
  Expected: a successful ExecuteStatement result for valid Aurora MySQL DDL.

Cause 3 (no row locking):
  data-api-row-lock-makes-a-native-nowait-lock-rejected-until-commit — psycopg.errors.SyntaxError: syntax error at or near "FOR"
  Expected: SELECT … FOR UPDATE NOWAIT parses; on contention it fails with the engine's lock-not-available error (SQLSTATE 55P03).

Cause 4 (binary parameters refused, wrong class):
  native-insert-is-read-by-the-data-api-postgres — psycopg.errors.InsufficientPrivilege: this endpoint reads parameters in the text format only
  typed-values-agree-between-the-data-api-and-a-native-session — psycopg.errors.InsufficientPrivilege: this endpoint reads parameters in the text format only
  Expected: the parameterised statement executes; no such refusal exists on Aurora, and a parameter-format fault is not SQLSTATE 42501.

Cause 5 (dirty reads):
  native-uncommitted-write-is-invisible-to-a-second-session-under-read-committed — AssertionError: session B must not see session A's uncommitted writes; got [(1, 'new'), (2, 'a-only')]
  Expected: session B reads only the committed row, i.e. [(1, 'old')].

## Steps to Reproduce
Replay the ten behavioral checks listed in trace_ids. In outline, against one Aurora cluster with the Data API enabled and its credentials in Secrets Manager:
1. Cross-surface: CREATE TABLE + INSERT via rds-data ExecuteStatement(resourceArn, secretArn), then SELECT the same table from a native psycopg session on the cluster's writer endpoint — it reports 'no such table'. Do it in the other direction (native CREATE, Data API SELECT) — ExecuteStatement reports relation does not exist, 42P01.
2. MySQL DDL: ExecuteStatement `CREATE TABLE t (id INT PRIMARY KEY, label VARCHAR(64) NOT NULL) ENGINE = InnoDB` against a MySQL cluster — DatabaseErrorException 'near "=": syntax error … at offset N'.
3. Row locking: from a native psycopg session run `SELECT … FROM t WHERE id = %s FOR UPDATE NOWAIT` — psycopg SyntaxError at or near "FOR".
4. Binary parameters: from psycopg3 execute a parameterised INSERT/SELECT with typed values (its default binary format) — InsufficientPrivilege 'this endpoint reads parameters in the text format only'.
5. Isolation: in native session A `BEGIN; UPDATE t SET label='new' WHERE id=1; INSERT INTO t VALUES (2,'a-only');` without committing, then SELECT from native session B — B sees both of A's uncommitted changes.
