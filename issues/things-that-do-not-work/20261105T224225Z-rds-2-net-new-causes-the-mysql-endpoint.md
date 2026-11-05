Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.mysql-auto-increment-utf8mb4-and-strict-overflow-rejected rds.mysql-require-secure-transport-plaintext-login-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.mysql-auto-increment-utf8mb4-and-strict-overflow-rejected|_rows#5|ProgrammingError|cur.execute(sql, params)
Issue-Key: rds.mysql-require-secure-transport-plaintext-login-rejected|_rows#13|ProgrammingError|cur.execute(sql, params)
Issue-Commit: 2730cd672dd608c80462ed6cac2962eb48b48789

# rds: 2 net-new causes — the MySQL endpoint refuses CREATE TABLE table options and @@GLOBAL/@@session system-variable reads with error 1064

> _Note: this issue was identified against the code base at commit `2730cd672dd608c80462ed6cac2962eb48b48789`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstance

## Description
This issue bundles 2 net-new root causes in the rds service, grounded in failing behavioural checks the suite marked neither ALREADY FILED nor OWNED. A pymysql session now opens on the published mysql endpoint and the engine now answers with MySQL-shaped 1064 syntax errors, so these are not the replay keys carried by the open issue 20261105T131136Z-rds-10-net-new-causes-the-engine-data.md (which recorded the earlier 1105 'near "X": syntax error' / 'this emulator's MySQL endpoint does not serve …' signatures). A fixer must close BOTH causes below before submitting.

=== Cause 1 of 2: CREATE TABLE table options (ENGINE=…, DEFAULT CHARSET=…) are refused with error 1064 ===
AWS: recorded behavior only (the engine surface is a MySQL contract, not a Smithy one). A MySQL DB instance accepts the table options every MySQL schema carries: `CREATE TABLE <t> (id INT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(5) NOT NULL, note VARCHAR(10)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4`. The table is created, AUTO_INCREMENT allocates ids, and the strict-mode data refusals that follow (1406 Data too long for column, 1264 Out of range value) are what a client branches on.
Emulator: the statement is refused at the table options — pymysql.err.ProgrammingError: (1064, "You have an error in your SQL syntax; check the manual that corresponds to your MySQL server version for the right syntax to use near '=InnoDB DEFAULT CHARSET=utf8mb4' at line 1"). The fixed text of a 1064 is MySQL's own sentence; its variable part is the fragment of the submitted statement at the error position — but AWS raises no error here, it creates the table.
Why: virtually every MySQL DDL script (mysqldump output, ORM migrations, CloudFormation-driven bootstrap SQL) carries ENGINE/CHARSET table options, so schema creation fails outright against an endpoint advertised as MySQL, and the AUTO_INCREMENT/strict-mode behaviour behind it is unreachable.
Fix: the engine data plane's MySQL statement parser — CREATE TABLE table_options (ENGINE, DEFAULT CHARSET/COLLATE, AUTO_INCREMENT=…) — and the per-table storage of those options.
Traces: rds.mysql-auto-increment-utf8mb4-and-strict-overflow-rejected.

=== Cause 2 of 2: system-variable reads (@@GLOBAL.<var> / @@session.<var>) are refused with error 1064 ===
AWS: recorded behavior only. A MySQL DB instance serves system variables as expressions: `SELECT @@GLOBAL.require_secure_transport` returns 1 when the applied DB parameter group sets require_secure_transport=1, and `SELECT @@session.<var>` returns the session value; this is also how a client observes a parameter-group change on the engine. With require_secure_transport ON, a plaintext login is refused by the server with error 3159 and the documented message "Connections using insecure transport are prohibited while --require_secure_transport=ON." (fixed text).
Emulator: the variable reference is refused at the qualifying dot — pymysql.err.ProgrammingError: (1064, "You have an error in your SQL syntax; check the manual that corresponds to your MySQL server version for the right syntax to use near '.require_secure_transport' at line 1") — so no system variable can be read, no parameter-group effect can be confirmed on the engine, and the 3159 TLS-enforcement refusal cannot be reached.
Why: a client cannot read any server variable (sql_mode, require_secure_transport, max_connections, time_zone …), so configuration introspection and every TLS/strict-mode behaviour keyed on it silently do not exist.
Fix: the engine data plane's MySQL expression parser (@@global./@@session./@@ variable references) wired to the instance's effective DB parameter group, and the require_secure_transport enforcement on the login path.
Traces: rds.mysql-require-secure-transport-plaintext-login-rejected.

## Evidence
Cause 1: pymysql.err.ProgrammingError: (1064, "You have an error in your SQL syntax; check the manual that corresponds to your MySQL server version for the right syntax to use near '=InnoDB DEFAULT CHARSET=utf8mb4' at line 1") for CREATE TABLE <t> (id INT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(5) NOT NULL, note VARCHAR(10)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4. AWS creates the table with no error.
Cause 2: pymysql.err.ProgrammingError: (1064, "You have an error in your SQL syntax; check the manual that corresponds to your MySQL server version for the right syntax to use near '.require_secure_transport' at line 1") for SELECT @@GLOBAL.require_secure_transport. AWS returns [(1,)] once the applied parameter group sets require_secure_transport=1, and refuses a plaintext login with error 3159 "Connections using insecure transport are prohibited while --require_secure_transport=ON."

## Steps to Reproduce
Cause 1: CreateDBInstance(Engine=mysql), poll to available, open a pymysql session on the published Endpoint, run CREATE TABLE t (id INT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(5) NOT NULL, note VARCHAR(10)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 and read the error.
Cause 2: CreateDBParameterGroup(family=mysql…), ModifyDBParameterGroup(Parameters=[{ParameterName:'require_secure_transport', ParameterValue:'1', ApplyMethod:'immediate'}]), ModifyDBInstance to apply it, then on a TLS session run SELECT @@GLOBAL.require_secure_transport (expect 1) and attempt a plaintext login (expect error 3159 with its documented message).
