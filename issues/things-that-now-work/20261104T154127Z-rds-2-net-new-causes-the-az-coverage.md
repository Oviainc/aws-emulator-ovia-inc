Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.db-subnet-group-insufficient-azs-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.db-subnet-group-insufficient-azs-rejected|run#7|AssertionError|assert message == (
Issue-Commit: ed43c7c5e13b0f1212f82cf25816ea1e565f9817

# rds: the AZ-coverage refusal names the AZs of the supplied subnets (cause 1 of 2; cause 2, the Aurora cluster endpoint, is split out and still open)

## Resolution

Cause 1 is FIXED. Cause 2 is NOT fixed, and it has been SPLIT OUT into its own open report
rather than counted as resolved here — its Issue-Key and its trace id moved with it, out of
this record's header, into
`issues/things-that-do-not-work/20261104T154127Z-rds-the-aurora-cluster-endpoint-admits-no-sql-session.md`.

* Cause 1 — FIXED. A subnet's Availability Zone is now read off the Amazon EC2 subnet record
  the request's identifier resolves to, asked over the in-process callback
  (`services/service-com-amazonaws-rds-amazonrdsv19/src/state/ec2_subnets.rs`, the contract
  in `services/common/src/network.rs`), and the group KEEPS the zone EC2 answered per subnet
  (`DbSubnetGroupRecord::subnet_zones`). The `DBSubnetGroupDoesNotCoverEnoughAZs` message
  therefore names the zones the request's own subnets cover, `DBSubnetGroup$Subnets` reports
  the same zones, and the coverage requirement is counted in distinct ZONES rather than in
  distinct identifiers, so two subnets of one zone no longer pass for two zones. The
  identifier-hashing helper that invented a zone is deleted. Pinned by
  `tests/all/db_subnet_group_ec2_zones.rs`, which mounts Amazon EC2 beside Amazon RDS on one
  engine so every id is one EC2 minted and every expected zone one EC2 reported.
* Cause 2 — NOT FIXED, and split out so it stays filed. An Aurora PostgreSQL cluster's
  endpoint admits no SQL session because no listener is opened for a cluster, and a service
  crate cannot open one that a recorded client could reach: the core's listener API binds a
  port of its own choosing (`vendor/server/src/listeners.rs`, `Listeners::tcp_plaintext`),
  while the client dials the engine's documented `DBCluster$Port`. See the split-out report
  for the blocker and what was learned.

> _Note: this issue was identified against the code base at commit `ed43c7c5e13b0f1212f82cf25816ea1e565f9817`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBSubnetGroup

## Description
Two distinct net-new root causes in the rds service, both grounded in failing behavioral checks that no filed issue carries the replay key for.

Cause 1 of 2: the DBSubnetGroupDoesNotCoverEnoughAZs refusal message names an Availability Zone that is not the AZ of the subnet the request supplied.
AWS: CreateDBSubnetGroup resolves every SubnetIds entry against EC2 and refuses a group that covers fewer than two AZs. Smithy (com.amazonaws.rds#AmazonRDSv19): CreateDBSubnetGroup models error DBSubnetGroupDoesNotCoverEnoughAZs, errorKind client, httpError 400, awsQueryError code DBSubnetGroupDoesNotCoverEnoughAZs/400; the projection it is derived from is DBSubnetGroup$Subnets[] of shape Subnet with member SubnetAvailabilityZone of shape AvailabilityZone (member Name). Recorded behavior for the message: 400, Code DBSubnetGroupDoesNotCoverEnoughAZs, Message "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: <AZs>. Add subnets to cover at least 2 AZs." The fixed text is that whole sentence pair exactly as quoted; the single variable part is <AZs>, the comma-separated list of Availability Zones the subnets named in SubnetIds actually cover, as EC2 DescribeSubnets reports them for those subnet ids. No value in that message is a per-run constant.
Emulator: the status and the error code are right, but the AZ it substitutes is not the AZ of the supplied subnet. The check resolves subnet_ids[0] through ec2.describe_subnets and compares; the emulator's message reads 'Current AZ coverage: us-west-2b.' while EC2 reports a different AZ for that very subnet id, so the message assertion fails on the AZ token alone. The emulator is evidently naming an AZ of its own choosing rather than resolving subnet -> AZ from EC2 state.
Why: a client reads 'Current AZ coverage' to learn which AZs its subnet list already covers and which one it must add. Being told an AZ its subnets are not in sends it to add a subnet it already has, and the same mis-resolution means a genuinely two-AZ subnet list can be refused while a one-AZ list is accepted — the rule that decides whether an instance can be created against the group at all.
Fix: the subnet -> AvailabilityZone resolution behind the CreateDBSubnetGroup handler's AZ-coverage precondition and the DBSubnetGroup/Subnet serializer in the rds service crate: the AZ list must come from the EC2 subnet records named in SubnetIds, not be assigned by the emulator. Open issue 20261104T053605Z-rds-5-net-new-causes-db-subnet-groups.md describes the same mis-resolution for the neighbouring checks rds.create-db-subnet-group-single-az-rejected / rds.subnet-group-insufficient-azs, and open issue 20261029T214632Z-rds-db-subnet-group-az-coverage-rule-is.md describes create/modify enforcement and wording; neither carries this check's replay key, so this cause is filed separately and should be closed together with them.
Traces: rds.db-subnet-group-insufficient-azs-rejected (mismatch, assert at check line 29).

Cause 2 of 2: an Aurora PostgreSQL cluster never reaches a state in which a SQL session can be opened on the endpoint it publishes, so the engine-truth check cannot execute a single statement.
AWS: CreateDBCluster(Engine='aurora-postgresql') plus a writer CreateDBInstance in that cluster converge to Status/DBInstanceStatus 'available' and publish DBCluster$Endpoint with DBCluster$Port 5432; a client that opens a TLS PostgreSQL session to that address:port with MasterUsername/MasterUserPassword against DatabaseName then observes PostgreSQL engine behavior — SQLSTATE 23505 on a UNIQUE violation naming the constraint, 23503 on a foreign-key violation, 55P03 lock_not_available for FOR UPDATE NOWAIT on a row another transaction holds, 40P01 deadlock_detected for exactly one victim of a two-session lock cycle, and ON DELETE CASCADE removing only the deleted parent's dependents. Smithy: CreateDBCluster output DBCluster$Endpoint/$ReaderEndpoint/$Port/$Status, DBCluster$MasterUsername, DBCluster$DatabaseName; the SQLSTATE values are recorded behavior only (PostgreSQL error-code appendix, cited by the check).
Emulator: the check never gets that far — the replay reports only 'Error: Check timed out after 60.0s' with no response to compare, meaning the sequence create_db_cluster -> create_db_instance -> poll to 'available' -> connect to cluster['Endpoint']:cluster['Port'] does not complete. Hypothesis: as with the already-filed MySQL sibling, the published cluster endpoint has no PostgreSQL wire-protocol listener, so the first-connect poll spins until the harness cap; a stalled convergence to 'available' for an aurora-postgresql writer would produce the same timeout. Either way the emulator announces an endpoint it does not serve on, which is its own divergence.
Why: a client that does the one thing an Aurora cluster exists for — connect and run SQL — gets nothing: no constraint refusal, no lock/deadlock semantics, no cascade behavior, and no way to tell from the control plane that the data plane is absent.
Fix: the rds crate's Aurora cluster data plane — a PostgreSQL wire-protocol listener bound to the address and port published in DBCluster$Endpoint/$Port with the cluster's MasterUsername/MasterUserPassword/DatabaseName wired to it — and, if convergence rather than the listener is what stalls, the aurora-postgresql writer's status progression to 'available' in the create_db_instance/create_db_cluster handlers. Open issue 20261104T053605Z-rds-the-published-db-instance-cluster-endpoint-serves.md describes this absence for 58 other checks, including the MySQL sibling rds.deep-rds-1-rds-engine-truth-mysql-constraint-lock-and-charset-violations-rejected, but does not carry this check's replay key.
Traces: rds.deep-rds-1-rds-engine-truth-aurora-postgres-constraint-and-lock-violations-rejected (mismatch, timeout at the connect/convergence poll).

## Evidence
Cause 1 — rds.db-subnet-group-insufficient-azs-rejected, verbatim replay output:
  File "check_rds_db_subnet_group_insufficient_azs_rejected.py", line 29, in run
    assert message == (
  AssertionError: The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: us-west-2b. Add subnets to cover at least 2 AZs.
The assertion prints the ACTUAL message. Expected: the identical sentence pair with 'Current AZ coverage: <AZ of subnet_ids[0] as ec2.describe_subnets reports it>'. The preceding assertion on the error code (DBSubnetGroupDoesNotCoverEnoughAZs) passed, so only the substituted AZ diverges. 'us-west-2b' is this run's value and is not itself the requirement — the requirement is the fixed sentence pair with the AZ list of the request's own subnets substituted.

Cause 2 — rds.deep-rds-1-rds-engine-truth-aurora-postgres-constraint-and-lock-violations-rejected, verbatim replay output:
  Error: Check timed out after 60.0s
Expected: after create_db_cluster(Engine='aurora-postgresql', DatabaseName='enginetruth', MasterUsername='sim2real') and a writer create_db_instance, both reach 'available', cluster['Port'] == 5432, and _try_connect(cluster['Endpoint'], 5432) returns 'connected' (psycopg, sslmode='require', SELECT 1 == [(1,)]); then the four engine beats observe SQLSTATE 23505 with constraint s2r_users_email_uq, 23503 with constraint s2r_child_parent_fk, 55P03 within 20 s for FOR UPDATE NOWAIT, and 40P01 for exactly one of two deadlocked sessions.
Actual: the check produces no response to compare at all — it is still inside the create/poll/first-connect sequence when the 60 s cap fires, so no SQL statement is ever executed against the published endpoint.

## Steps to Reproduce
Cause 1:
1. Boot the emulator from this checkout.
2. ec2.describe_subnets over the run's VPC and note the AvailabilityZone of one subnet, e.g. subnet S in us-west-2a.
3. rds.create_db_subnet_group(DBSubnetGroupName=<new>, DBSubnetGroupDescription=<any>, SubnetIds=[S]).
4. Read the 400 DBSubnetGroupDoesNotCoverEnoughAZs message: 'Current AZ coverage:' names an AZ other than S's AZ from step 2.

Cause 2:
1. rds.create_db_cluster(DBClusterIdentifier=<id>, Engine='aurora-postgresql', EngineVersion=<default>, MasterUsername='sim2real', MasterUserPassword=<pw>, DatabaseName='enginetruth', DBSubnetGroupName=<sng>, VpcSecurityGroupIds=[<sg>], BackupRetentionPeriod=1).
2. rds.create_db_instance(DBInstanceIdentifier=<id>-w, DBClusterIdentifier=<id>, Engine='aurora-postgresql', DBInstanceClass=<orderable small class>, PubliclyAccessible=False).
3. Poll describe_db_instances / describe_db_clusters for 'available', then read DBCluster.Endpoint and DBCluster.Port (expect 5432).
4. Open a PostgreSQL session to that address:port with the master credentials and dbname 'enginetruth' (psycopg, sslmode='require') and run SELECT 1 — no session is ever established within the check budget.
