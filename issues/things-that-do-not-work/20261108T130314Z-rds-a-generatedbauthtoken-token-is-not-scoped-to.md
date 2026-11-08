Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.iam-db-auth-token-connects-and-maps-to-the-granted-role
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.iam-db-auth-token-connects-and-maps-to-the-granted-role|run#18|AssertionError|assert token.startswith(f"{addr}:{port}/"), (
Issue-Commit: 250d476ecb357920d67b44d09bfeff057fcec9e1

# rds: a GenerateDBAuthToken token is not scoped to the DB instance endpoint DescribeDBInstances publishes, so IAM database authentication cannot be exercised

> _Note: this issue was identified against the code base at commit `250d476ecb357920d67b44d09bfeff057fcec9e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBInstances

## Description
This issue carries 1 net-new root cause in the rds service, grounded in the one failing behavioural check in this suite report that was marked neither ALREADY FILED nor OWNED by an open issue. A fixer must close the single cause below before submitting.

=== Cause 1 of 1: the rds-db:connect token minted for an instance's published endpoint is not scoped to that endpoint ===

AWS: Smithy (com.amazonaws.rds): DBInstance$Endpoint is an `Endpoint`; `Endpoint$Address` — "Specifies the DNS address of the DB instance"; `Endpoint$Port` (Integer) — "Specifies the port that the database engine is listening on". `CreateDBInstanceMessage$Port` defaults to the engine family's port (5432 for RDS for PostgreSQL) when omitted, and `DBInstance$IAMDatabaseAuthenticationEnabled` reports that IAM auth is on. RDS mints ONE DNS name per instance, derived from that instance's own DBInstanceIdentifier, and that name on the engine's port is the endpoint a client dials. The rds-db connect token is a client-side SigV4 presign (signing name `rds-db`) over `<DBHostname>:<Port>/?Action=connect&DBUser=<user>&X-Amz-Algorithm=AWS4-HMAC-SHA256&…`, so the token is literally PREFIXED by the host:port it was minted for, and AWS's documented recipe is: read `Endpoint.Address`/`Endpoint.Port` from DescribeDBInstances, mint a token for exactly that pair, and present it as the password over TLS. The engine accepts it only for a role granted `rds_iam`, and only when the token is scoped to the endpoint being dialled. Consequence: a token minted with DBHostname=`Endpoint.Address` and Port=`Endpoint.Port` of a given instance must begin with `<Endpoint.Address>:<Endpoint.Port>/` and must then authenticate on that same endpoint.

Emulator: for a postgres instance created with `EnableIAMDatabaseAuthentication=true`, the token minted for the instance's own published endpoint comes back scoped to a DIFFERENT host:port than the `Endpoint.Address`/`Endpoint.Port` pair the same DescribeDBInstances response publishes for that instance, so `token.startswith(f"{Address}:{Port}/")` is false and the IAM-auth leg of the check never runs. Observed token prefix: `127.0.0.1:38349/?Action=connect&DBUser=iamuser&X-Amz-Algorithm=AWS4-HMAC-SHA256&` — a loopback host and an ephemeral, per-instance port rather than the instance's AWS-form per-instance DNS name on the engine's port. The loopback literal and the particular port number are environment-specific and are NOT the requirement; the requirements are (a) `Endpoint.Address` is the per-instance DNS name derived from the DB instance identifier and `Endpoint.Port` is the engine's port (5432 for postgres when `Port` is omitted), and (b) the endpoint the token is scoped to is the endpoint DescribeDBInstances publishes. Hypothesis (neither the model nor the recording states the mechanism): the endpoint the emulator publishes in the DBInstance envelope and the socket its engine data plane is actually reachable on are two different values, so a token minted for one is not scoped to the other.

Why: IAM database authentication is unusable end-to-end against this emulator. A client that follows AWS's documented sequence — DescribeDBInstances, read `Endpoint.Address`/`Endpoint.Port`, mint a token for that endpoint, connect with the token as the password with `sslmode=require` — holds a token whose scope does not match the endpoint it dials, so the engine must refuse it; and any client that validates the token it received (every SDK sample asserts the token names the endpoint it was minted for) rejects it before even connecting. The same divergence breaks CNAMEs, connection strings, security-group rules and proxy targets keyed on the published endpoint, because the published endpoint does not identify where the instance is served.

Fix: the DB-instance endpoint projection in the rds crate's CreateDBInstance/DescribeDBInstances serializers and the instance state that records the endpoint address and port — publish a per-instance `Endpoint.Address` derived from the instance's own identifier and the engine's port (or the requested `Port`) in `Endpoint.Port`, and make that published pair the endpoint the engine data plane serves, so a token presigned for the published endpoint is the token the engine's IAM-auth acceptance path validates.

Traces: rds.iam-db-auth-token-connects-and-maps-to-the-granted-role.

Related open issues that do NOT cover this check (their replay keys do not include it, and their causes are about the published port/address values alone, not about the token scoping): 20261108T051751Z-rds-7-net-new-causes-endpoint-port-is.md (Cause 1, Endpoint.Port is an ephemeral listener port), 20261106T031005Z-rds-7-net-new-causes-endpoint-port-publishes.md (Cause 1), 20261105T224225Z-rds-2-net-new-causes-every-db-instance.md (every instance publishes the same Endpoint.Address), and 20261105T090000Z-rds-a-db-instance-publishes-the-listeners-port-not-the-requested-port.md.

## Evidence
Failing check: rds.iam-db-auth-token-connects-and-maps-to-the-granted-role (verdict: mismatch).

Assertion that diverged (check_rds_iam_db_auth_token_connects_and_maps_to_the_granted_role.py:251):
    addr = inst["Endpoint"]["Address"]; port = inst["Endpoint"]["Port"]
    token = rds.generate_db_auth_token(DBHostname=addr, Port=int(port), DBUsername="iamuser", Region=ctx.region)
    assert token.startswith(f"{addr}:{port}/")

Expected: the minted token begins with the published endpoint pair, i.e. `<Endpoint.Address>:<Endpoint.Port>/?Action=connect&DBUser=iamuser&X-Amz-Algorithm=AWS4-HMAC-SHA256&…`, where `Endpoint.Address` is the instance's own per-instance RDS DNS name and `Endpoint.Port` is 5432 for a postgres instance created with `Port` omitted; the token then authenticates a TLS session whose `SELECT current_user` is the rds_iam-granted role and whose `pg_stat_ssl` row reports ssl = true.

Actual: AssertionError: the auth token must be scoped to the endpoint it was minted for; it begins '127.0.0.1:38349/?Action=connect&DBUser=iamuser&X-Amz-Algorithm=AWS4-HMAC-SHA256&'

No HTTP error status or error code is involved: CreateDBInstance/DescribeDBInstances answered 200 and reported `IAMDatabaseAuthenticationEnabled: true`; the divergence is in the `Endpoint` the DBInstance envelope publishes versus the endpoint the token is scoped to. The literal host `127.0.0.1` and the port `38349` are this run's values and fill the template only — the requirement is the per-instance DNS name and the engine port, and that the token's scope equals the published endpoint.

## Steps to Reproduce
1. CreateDBSubnetGroup over two private subnets in the shared VPC; create a security group admitting 5432.
2. CreateDBInstance(DBInstanceIdentifier=<id>, Engine="postgres", EngineVersion=<the region's DefaultOnly version>, DBInstanceClass=<smallest orderable class>, AllocatedStorage=20, MasterUsername=<master>, MasterUserPassword=<pw>, DBName=<db>, BackupRetentionPeriod=0, PubliclyAccessible=false, VpcSecurityGroupIds=[<sg>], DBSubnetGroupName=<group>, EnableIAMDatabaseAuthentication=true).
3. Poll DescribeDBInstances until DBInstanceStatus == "available". Assert IAMDatabaseAuthenticationEnabled is true, and record Endpoint.Address and Endpoint.Port. Endpoint.Address must be the instance's own RDS DNS name derived from <id> (not a shared or loopback host) and Endpoint.Port must be 5432 because Port was omitted.
4. Connect as the master user and run: CREATE USER iamuser WITH LOGIN; GRANT rds_iam TO iamuser; verify the grant via pg_auth_members.
5. Mint a token: rds.generate_db_auth_token(DBHostname=<Endpoint.Address>, Port=<Endpoint.Port>, DBUsername="iamuser", Region=<region>). Assert the token begins with "<Endpoint.Address>:<Endpoint.Port>/" and contains "DBUser=iamuser". This is the step that fails today.
6. Open a postgres connection to <Endpoint.Address>:<Endpoint.Port> with user=iamuser, password=<the freshly minted token>, sslmode=require, and assert SELECT current_user returns 'iamuser', SELECT 1 returns 1, and SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid() returns true.
