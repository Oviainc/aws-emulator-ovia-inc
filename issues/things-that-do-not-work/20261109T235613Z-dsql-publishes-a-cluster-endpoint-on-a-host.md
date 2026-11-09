Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dsql.connection-without-tls-or-admin-token-rejected dsql.create-stream-lifecycle-to-kinesis dsql.dsql-occ-write-conflict-rejects-the-losing-commit dsql.expired-auth-token-rejected-open-session-survives dsql.iam-mapped-database-role-privileges-denied dsql.locking-read-conflicts-rejected-at-commit dsql.native-sql-round-trip-create-insert-select dsql.sequence-and-identity-cache-rule-rejected dsql.transaction-constraints-rejected dsql.unsupported-postgres-statements-rejected
Issue-Shape-Id: com.amazonaws.dsql#DSQL
Issue-Key: dsql.connection-without-tls-or-admin-token-rejected|_connect#5|PublishedUrlRejected|db_host, port = ctx.endpoint_host(host, 5432)
Issue-Key: dsql.create-stream-lifecycle-to-kinesis|run#33|PublishedUrlRejected|address, port = ctx.endpoint_host(active["endpoint"], 5432)
Issue-Key: dsql.dsql-occ-write-conflict-rejects-the-losing-commit|run#7|PublishedUrlRejected|address = ctx.endpoint_host(endpoint, 5432)
Issue-Key: dsql.expired-auth-token-rejected-open-session-survives|_connect#4|PublishedUrlRejected|db_host, port = ctx.endpoint_host(host, 5432)
Issue-Key: dsql.iam-mapped-database-role-privileges-denied|_connect#13|PublishedUrlRejected|db_host, port = ctx.endpoint_host(host, 5432)
Issue-Key: dsql.locking-read-conflicts-rejected-at-commit|_connect#6|PublishedUrlRejected|db_host, port = ctx.endpoint_host(host, 5432)
Issue-Key: dsql.native-sql-round-trip-create-insert-select|_connect_with_retry#12|PublishedUrlRejected|db_host, port = ctx.endpoint_host(endpoint, 5432)
Issue-Key: dsql.sequence-and-identity-cache-rule-rejected|_connect#6|PublishedUrlRejected|db_host, port = ctx.endpoint_host(host, 5432)
Issue-Key: dsql.transaction-constraints-rejected|_connect#5|PublishedUrlRejected|db_host, port = ctx.endpoint_host(host, 5432)
Issue-Key: dsql.unsupported-postgres-statements-rejected|_connect#5|PublishedUrlRejected|db_host, port = ctx.endpoint_host(host, 5432)
Issue-Commit: 798119d59e6f7ad23ed03b3274703a15fb5570c4

# dsql publishes a cluster endpoint on a host the emulator does not serve, so every data-plane check is refused before it can connect

> _Note: this issue was identified against the code base at commit `798119d59e6f7ad23ed03b3274703a15fb5570c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Cause 1 of 1: the `endpoint` the DSQL control plane publishes for a cluster is synthesised as Aurora DSQL's production DNS name instead of an address this emulator is reached on, so every check that takes the published endpoint and connects to it is refused as an egress to a foreign host and the whole data-plane surface (native SQL, auth-token handling, transaction/OCC semantics, CDC stream lifecycle) is never exercised at all.

AWS: Smithy (services/service-com-amazonaws-dsql-dsql/smithy-model/com-amazonaws-dsql-dsql.json; smithy_query query="operation" operation="CreateCluster,GetCluster"): CreateCluster POST /cluster -> 200 and GetCluster GET /cluster/{identifier} -> 200 both carry output member `endpoint`, target shape Endpoint, pattern ^[a-zA-Z0-9.-]+$, not required. The model constrains only the character set, so the requirement is structural and not a literal string: `endpoint` is the DNS name at which a client can afterwards reach THAT cluster's PostgreSQL data plane on the responding deployment. On real AWS the value names a host the responding deployment owns and accepts connections on, and the IAM auth tokens GenerateDbConnectAdminAuthToken / GenerateDbConnectAuthToken are presigned for exactly that hostname. No error code or message is involved: these are 200 successes, and the diverging thing is the value of a member in the success body.

Emulator: CreateCluster returns 200 with `endpoint` of the form `{identifier}.dsql.{region}.on.aws` (replay examples: 01m71bfyhnxcxnpd5wdk5f431h.dsql.us-west-2.on.aws, 01m71bkgacakwsbj4s6faenjh7.dsql.us-west-2.on.aws — the 26-char label is the minted identifier and `us-west-2` is the request region, so only the `.dsql.<region>.on.aws` suffix is invariant), and GetCluster echoes the same value. That name is in AWS's production DNS zone, which the emulator neither owns nor is reached on, and it is derived purely from region+identifier with no reference to the host/port the request actually arrived on. All ten checks below take the endpoint from the control plane, hand it to the harness as `endpoint_host`, and are refused with `simparity.published.PublishedUrlRejected: endpoint_host was given <identifier>.dsql.us-west-2.on.aws:5432 (published by the emulator or built by the check): its host is not one of the emulator's own hosts`.

Why: a client following the documented flow — CreateCluster, poll until ACTIVE, connect to the returned `endpoint` — is pointed off the emulator entirely. The name either fails to resolve or resolves to real Aurora DSQL (an egress out of the emulated account), and in neither case reaches the cluster just created. Any driver, psql, or SDK that treats the response endpoint as authoritative (its only purpose) cannot be made to work against the emulator, and an admin/DbConnect auth token presigned for that hostname is scoped to a host the emulator does not serve, so even a data plane added later would reject it. Consequently none of the emulator's DSQL data-plane behaviour — TLS enforcement, admin-vs-DbConnect token mapping, token expiry, OCC and locking conflict rejection at commit, unsupported-statement and sequence/identity rejections, CDC stream delivery — can be reached or validated.

Fix: the endpoint derivation in the DSQL service's state (the single helper both the CreateCluster handler and every cluster read path use) must render the address the emulator is actually reachable at — the host and port the request was served on — rather than interpolating AWS's `{identifier}.dsql.{region}.on.aws` template, and every read path reporting a stored cluster must report that same address so the published value and the served address cannot drift. The concrete host is environment-specific and is not itself a requirement; what is required is that it name this emulator and accept a PostgreSQL connection for that cluster. Related open issues: 20261109T163226Z-dsql-createcluster-publishes-a-cluster-endpoint-on-a.md files the same root cause from the WIRE corpus (replay key dsql|CreateCluster|2xx->2xx|$.endpoint) and does not carry any of these behavioral check ids; 20261107T211056Z-dsql-the-published-cluster-endpoint-accepts-no-postgresql.md, 20261023T121409Z-dsql-clusters-report-active-with-a-published-endpoint.md and 20261022T193920Z-dsql-publishes-a-cluster-endpoint-but-serves-no.md describe the adjacent, distinct defect that no PostgreSQL listener answers behind the published endpoint — a listener placed behind an address the emulator never advertises would still leave these ten checks refused. None of them is marked as owning these checks in the replay report.

Traces: the ten failing checks listed in trace_ids, all refused at the same point with the same PublishedUrlRejected message. File citations: services/service-com-amazonaws-dsql-dsql/src/state.rs lines 1622-1627 (`cluster_endpoint(region, identifier)` -> format!("{identifier}.dsql.{region}.on.aws")); services/service-com-amazonaws-dsql-dsql/src/ops/create_cluster.rs (endpoint reported in the 200 output); services/service-com-amazonaws-dsql-dsql/src/ops/get_cluster.rs (same value echoed on read).

## Evidence
Ten behavioral checks under service heading `### dsql` fail (verdict mismatch) at the moment they use the published cluster endpoint:

  File "context.py", line 619, in endpoint_host
    self._published_target(f"{published_host}:{port}", "endpoint_host")
  File "context.py", line 586, in _published_target
    raise PublishedUrlRejected(f"{message}: {problem[1]}", problem[0], value_class(value))
  simparity.published.PublishedUrlRejected: endpoint_host was given 01m71bfyhnxcxnpd5wdk5f431h.dsql.us-west-2.on.aws:5432 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]: the endpoint host, a documented own host or, for a loopback endpoint, a loopback address

The identical message appears for each of the ten checks, differing only in the 26-character cluster identifier (01m71bkgacakwsbj4s6faenjh7, 01m71bg9yszqjmwahjp4jfazy4, 01m71bf4pc1hp1zbq44dvf5vvw, 01m71bhvkcb6pfk4vgpa8znpp9, 01m71bmfs75czwwtnm62wderrw, 01m71bg483kq4z6nt6f3tmt73x, 01m71bn6gmw9r1vksy9zk2qqw8, 01m71bgv15d88ybaepe3n52xy2, 01m71bh6dqcfah74crfrtvhrqw) — a per-run value, not part of the requirement.

Expected (AWS): CreateCluster/GetCluster 200 with `endpoint` naming a host the responding deployment serves, reachable on the PostgreSQL port for that cluster; the literal hostname is not asserted.
Actual (emulator): CreateCluster/GetCluster 200 with `endpoint` = "<minted-identifier>.dsql.<request-region>.on.aws", a host in AWS's production zone that the emulator does not own and is not reached on; the fixed part of the divergence is the `.dsql.<region>.on.aws` suffix and the absence of any relationship to the address the request was served on.
Status/error code/message: no error is returned; the divergence is a member value in a 200 success body. The downstream consequence is that zero DSQL data-plane assertions (e.g. the expected refusals "unable to accept connection, SSL is mandatory. AWS Authentication is required." and "unable to accept connection, access denied" with hint "Wrong user to action mapping. user: admin, action: DbConnect") are ever evaluated.

## Steps to Reproduce
1. Point an AWS DSQL client at the emulator's endpoint.
2. Call CreateCluster (POST /cluster), e.g. with deletionProtectionEnabled=false and tags.
3. Read `endpoint` from the 200 response: it is `<new-identifier>.dsql.<request-region>.on.aws`.
4. Compare that host with the host[:port] the emulator is actually being reached on: they differ — the published name is in AWS's production DNS zone, not the emulator's.
5. Call GetCluster(identifier=<new-identifier>) and poll to ACTIVE: the same foreign hostname is echoed.
6. Generate an admin auth token for that hostname and attempt a psycopg connection to it on port 5432: the connection leaves the emulator (or fails to resolve) and never reaches the cluster just created; the behavioral harness refuses it up front with PublishedUrlRejected because the announced host is not one of the emulator's own hosts.
7. Re-run the ten checks in trace_ids; each fails at the same point.
