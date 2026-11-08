Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.deep-rds-1-rds-engine-truth-aurora-postgres-constraint-and-lock-violations-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.deep-rds-1-rds-engine-truth-aurora-postgres-constraint-and-lock-violations-rejected|_retry_delete#6|Timeout|fn()
Issue-Commit: 8a19514b68a1f686dd21900920ede54cd81f71e4

# rds: the aurora-postgresql cluster+writer engine-truth flow never becomes usable — the check reaches no SQL assertion before its 180 s budget expires

> _Note: this issue was identified against the code base at commit `8a19514b68a1f686dd21900920ede54cd81f71e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
Cause 1 of 1: an aurora-postgresql cluster plus its writer instance, created and then polled to `available` and connected to over TLS on the cluster endpoint, never gets far enough for a single engine assertion to run; the check ends on its wall-clock budget with no response recorded at all. Failing check id: rds.deep-rds-1-rds-engine-truth-aurora-postgres-constraint-and-lock-violations-rejected.

AWS: CreateDBCluster(Engine=aurora-postgresql) followed by CreateDBInstance(DBClusterIdentifier=...) yields a writer whose DBInstanceStatus becomes `available` and a cluster whose Status becomes `available`, whose `Endpoint`/`Port` publish a reachable PostgreSQL listener on port 5432, and which then answers SQL: a duplicate key on a UNIQUE constraint is refused with SQLSTATE 23505 naming the constraint, an orphan child row with 23503 naming the FK, `FOR UPDATE NOWAIT` on a row another transaction holds with 55P03 immediately, and a two-session lock cycle victimises exactly one session with 40P01 while the survivor commits. Reference: PostgreSQL error-code appendix (https://www.postgresql.org/docs/current/errcodes-appendix.html). The RDS Smithy model states only the control-plane side of this (CreateDBCluster/CreateDBInstance -> DBCluster.Status, DBInstance.DBInstanceStatus, DBCluster.Endpoint/Port); the SQLSTATE behavior is recorded behavior only.

Emulator: nothing was recorded — the replay reports `Check timed out after 180.0s`, i.e. the check produced no assertion failure and no response, so the aurora-postgresql provisioning-then-connect prefix (create cluster, create writer, poll both to `available`, open a TLS session on the published cluster endpoint) does not complete in the time the suite allows a check. Hypothesis (neither modelled nor recorded, stated as such): the writer/cluster does not reach `available` promptly, or the published cluster endpoint does not accept a TLS PostgreSQL session, so the first poll consumes the whole budget. Note the sibling MySQL check of the same group gets one step further and fails on the published port, and the published-port defect for aurora-postgresql/postgres is already described by the open issue 20261108T051751Z-rds-7-net-new-causes-endpoint-port-is.md; that issue does not carry this check's replay key, and this check never reaches the port assertion, so it is filed here.

Why: a client relying on AWS's behavior treats an aurora-postgresql cluster as ready for SQL shortly after its writer reports `available`, and relies on the engine to enforce constraints and locks. Against the emulator the whole data-plane contract of an Aurora PostgreSQL cluster is unverifiable: no constraint, FK, NOWAIT or deadlock rejection can be exercised, so applications that depend on 23505/23503/55P03/40P01 (or merely on connecting) cannot be emulated at all.

Fix: the RDS aurora-postgresql create/transition path and the PostgreSQL data-plane listener it fronts — the cluster and writer state machine that must drive DBInstanceStatus/Status to `available`, and the published cluster-endpoint listener that must accept a TLS session on 5432 and serve the engine's SQLSTATE rejections. File and line citations belong with that handler, not here.

Traces: rds.deep-rds-1-rds-engine-truth-aurora-postgres-constraint-and-lock-violations-rejected (as the example; the parallel MySQL check rds.deep-rds-1-rds-engine-truth-mysql-constraint-lock-and-charset-violations-rejected is owned elsewhere and is not part of this issue).

## Evidence
Expected (AWS): the check creates an aurora-postgresql cluster and writer, both reach `available`, DescribeDBClusters reports Port 5432, a TLS session opens on the cluster endpoint, and the engine then refuses a duplicate UNIQUE key with SQLSTATE 23505 (constraint `s2r_users_email_uq`), an orphan FK insert with 23503 (`s2r_child_parent_fk`), `SELECT ... FOR UPDATE NOWAIT` on a held row with 55P03 in under 20 s, and the deadlock victim with 40P01 while the other session commits.

Actual (emulator): no assertion is ever reached. The replay verdict is `[mismatch] rds.deep-rds-1-rds-engine-truth-aurora-postgres-constraint-and-lock-violations-rejected` with detail `Error: Check timed out after 180.0s` — no status, no error code and no SQL response was produced within the budget.

## Steps to Reproduce
1. CreateDBCluster(Engine=aurora-postgresql, EngineVersion=<default from DescribeDBEngineVersions>, MasterUsername=sim2real, DatabaseName=enginetruth, DBSubnetGroupName=<private subnets>, VpcSecurityGroupIds=[<sg>], BackupRetentionPeriod=1, DeletionProtection=false).
2. CreateDBInstance(DBInstanceIdentifier=<cluster>-w, DBClusterIdentifier=<cluster>, Engine=aurora-postgresql, DBInstanceClass=<smallest orderable>, PubliclyAccessible=false).
3. Poll DescribeDBInstances until DBInstanceStatus == 'available', then DescribeDBClusters until Status == 'available'; read Endpoint and Port.
4. Open a psycopg TLS session (sslmode=require) to that endpoint on the published port as the master user against database `enginetruth` and run the constraint/lock beats.
   AWS: every step completes and the four rejections are observed.
   Emulator: the sequence does not complete — the check exhausts its 180 s budget with no response.
