Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.oracle-admin-user-contract rds.oracle-rollback-unique-constraint-and-dynamic-parameter-apply
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.oracle-admin-user-contract|run#12|PublishedUrlRejected|host, port = ctx.endpoint_host(inst["Endpoint"]["Address"], int(inst["Endpoint"]["Port"]))
Issue-Key: rds.oracle-rollback-unique-constraint-and-dynamic-parameter-apply|run#17|PublishedUrlRejected|host, port = ctx.endpoint_host(inst["Endpoint"]["Address"], int(inst["Endpoint"]["Port"]))
Issue-Commit: 424434639e8b1acdb0579cf5577ef0e9dbf90686

# rds: an Oracle DB instance publishes Endpoint.Address as a real *.rds.amazonaws.com host the emulator does not serve, so no Oracle data plane is reachable

> _Note: this issue was identified against the code base at commit `424434639e8b1acdb0579cf5577ef0e9dbf90686`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstance

## Description
Cause 1 of 1: Oracle DB instances publish an off-emulator endpoint address.

AWS: Smithy (com.amazonaws.rds) — CreateDBInstance/DescribeDBInstances return DBInstance.Endpoint of shape com.amazonaws.rds#Endpoint with members Address ("Specifies the DNS address of the DB instance"), Port ("Specifies the port that the database engine is listening on") and HostedZoneId. On AWS that Address is a DNS name that actually resolves to, and accepts TCP connections on, the listening Oracle TNS listener for the instance, and Port is 1521 by default. The whole point of the member is that a client takes Address:Port and connects: the recorded behavior is an oracledb client connecting to Address:Port and completing a login as the master user.

Emulator: for engine `oracle-*` the emulator synthesises an AWS-shaped hostname of the form <db-instance-identifier>.<12-digit-account>.<region>.rds.amazonaws.com and publishes it in Endpoint.Address with Port 1521 — an address that is NOT one of the emulator's own hosts and that nothing in the emulator listens on. The harness refuses the published target before any TNS traffic is attempted:

  simparity.published.PublishedUrlRejected: endpoint_host was given <id>.<account>.<region>.rds.amazonaws.com:1521 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]: the endpoint host, a documented own host or, for a loopback endpoint, a loopback address

The fixed text here is the PublishedUrlRejected diagnosis; the variable parts are the generated instance identifier, the account id and the region, which differ per run and are not part of the requirement. The requirement is only that Endpoint.Address be an address the emulator itself serves.

Why: a client that does exactly what AWS documents — read Endpoint.Address/Endpoint.Port from CreateDBInstance/DescribeDBInstances and open an Oracle connection to it — is handed an address that either does not resolve or, worse, resolves to real AWS. Every Oracle behavior that depends on a live session is therefore unreachable: the admin-user identity/privilege contract (who CURRENT_USER/SYS_CONTEXT reports, which system privileges the RDS master user must and must not hold) and transaction/parameter semantics (ROLLBACK discarding uncommitted rows, a unique-constraint violation, a dynamic parameter such as cursor_sharing taking effect in a new session). None of these can be exercised at all, so the emulator's Oracle engine fidelity is wholly unverified.

Fix: the DB-instance endpoint projection/state for the Oracle engines — the component that mints Endpoint.Address when an instance is created and that answers DescribeDBInstances — must publish the emulator's own host and the port its Oracle listener is bound to, and the emulator must actually stand up an Oracle-protocol listener on that address, the same way it does for the engines whose native endpoints are already served. Related but NOT the same work item: open issue 20261109T135215Z-rds-10-net-new-causes-reader-proxy-sql.md covers reader/proxy/SQL Server endpoints being unreachable AWS hostnames and ignored requested ports; it does not carry these two Oracle checks' replay keys, and the suite report does not mark them as owned by it.

Traces: rds.oracle-admin-user-contract and rds.oracle-rollback-unique-constraint-and-dynamic-parameter-apply, both failing in context.py endpoint_host with PublishedUrlRejected on the published <...>.rds.amazonaws.com:1521 address.

## Evidence
Expected (AWS): CreateDBInstance(Engine="oracle-*")/DescribeDBInstances return Endpoint.Address = a DNS name that resolves to the instance's live TNS listener and Endpoint.Port = 1521; an oracledb client connects to Address:Port and authenticates as the master user, then exercises CURRENT_USER/privilege checks, ROLLBACK of an uncommitted INSERT, a unique-constraint violation, and a dynamic parameter change visible in a new session.

Actual (emulator): Endpoint.Address is a synthesised real-AWS-shaped host the emulator does not serve, and the published target is rejected before any connection:

  File "context.py", line 619, in endpoint_host
    self._published_target(f"{published_host}:{port}", "endpoint_host")
  File "context.py", line 586, in _published_target
    raise PublishedUrlRejected(f"{message}: {problem[1]}", problem[0], value_class(value))
  simparity.published.PublishedUrlRejected: endpoint_host was given sim2real-behav-ora-0f7f1da29690.000000000001.us-west-2.rds.amazonaws.com:1521 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]: the endpoint host, a documented own host or, for a loopback endpoint, a loopback address

The second check fails identically with sim2real-behav-ora-133357c15744.000000000001.us-west-2.rds.amazonaws.com:1521. The instance identifier, account id and region in those strings are per-run values; the defect is that the host is not one the emulator serves.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. rds.CreateDBInstance with an Oracle engine (e.g. Engine="oracle-se2", DBName="ORCL", MasterUsername="sim2real", a db.* class and allocated storage), then wait for status "available".
3. Read DBInstance.Endpoint.Address and DBInstance.Endpoint.Port from DescribeDBInstances.
4. Observe Address is <identifier>.<account>.<region>.rds.amazonaws.com and Port is 1521 — not an address the emulator listens on.
5. Attempt an Oracle (oracledb) connection to Address:Port as the master user: nothing on the emulator serves that host, so the session can never be opened.
6. Replay rds.oracle-admin-user-contract and rds.oracle-rollback-unique-constraint-and-dynamic-parameter-apply to confirm.
