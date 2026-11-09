Issue-Source: trace-sweep-divergence
Issue-Trace: dsql-cdc-stream-error-handling
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dsql-cdc-stream-error-handling dsql-cluster-policy-workflow dsql-vpc-endpoint-service-name
Issue-Shape-Id: com.amazonaws.dsql#DSQL
Issue-Key: dsql|CreateCluster|2xx->2xx|$.endpoint
Issue-Commit: 545841f07feb57b92e392d55db412e521f655a7b

# dsql CreateCluster publishes a cluster endpoint on a host the emulator does not serve

> _Note: this issue was identified against the code base at commit `545841f07feb57b92e392d55db412e521f655a7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Cause 1 of 1: CreateCluster's `endpoint` member is rendered as Aurora DSQL's production hostname form instead of an address the emulator itself answers on, so the replay flags it `host_not_own`.

AWS: Smithy (services/service-com-amazonaws-dsql-dsql/smithy-model/com-amazonaws-dsql-dsql.json, smithy_query query="operation" operation="CreateCluster"): CreateCluster POST /cluster -> 200, output member `endpoint` targets shape Endpoint, pattern ^[a-zA-Z0-9.-]+$, not required; the required members are identifier, arn, status, creationTime, deletionProtectionEnabled. The model places no further constraint on the hostname's value, and the fixture does not assert a literal one (expected '<shaped-not-asserted>'): the requirement is structural, namely that `endpoint` is the DNS name at which a client can afterwards reach that cluster's data plane. On real AWS the value it carries is the address of the serving endpoint, i.e. a host the responding deployment owns and accepts connections on. The step is a 200 success; no error code or message is involved.

Emulator: the 200 body carries `endpoint` of the form `{identifier}.dsql.{region}.on.aws` (e.g. '01m70j916bxzay57y39x6wdey8.dsql.us-west-2.on.aws'), a name in AWS's own production DNS zone which this emulator neither owns nor is reached on — the replay's host-ownership check reports `$.endpoint: host_not_own`. The value is synthesised unconditionally from the request's region and the minted identifier, with no reference to the host/port the request actually arrived on, so every cluster created through any entry point advertises a foreign address. The same cluster's GetCluster read echoes it.

Why: a client that follows the documented flow — CreateCluster, wait for ACTIVE, then connect to the returned `endpoint` — is pointed off the emulator entirely. Depending on the environment the name either fails to resolve or resolves to real Aurora DSQL, so the connect attempt leaves the emulated account (egress), and in neither case does it reach the cluster that was just created. Any SDK, driver or tooling that treats the response's endpoint as authoritative (which is the only thing it is for) cannot be made to work against the emulator, and an IAM auth token presigned for that hostname is scoped to a host the emulator is not serving.

Fix: the endpoint derivation in the DSQL service's state — the one helper both the CreateCluster handler and the GetCluster read use — must render an address the emulator is actually reachable at (the host and port the request was served on), rather than interpolating AWS's `{identifier}.dsql.{region}.on.aws` template. Both the create response and every read path that reports a stored cluster must report that same address, so the published value and the address the emulator answers on cannot drift apart. The hostname's concrete value is environment-specific and is not itself a requirement; what is required is that it name this emulator.

Note for triage: the open behavioral issues 20261107T211056Z-dsql-the-published-cluster-endpoint-accepts-no-postgresql.md, 20261023T121409Z-dsql-clusters-report-active-with-a-published-endpoint.md and 20261022T193920Z-dsql-publishes-a-cluster-endpoint-but-serves-no.md describe the absence of a PostgreSQL listener behind the published endpoint; none of them carries this wire step's replay key, and this divergence is the adjacent but distinct defect that the advertised hostname is not the emulator's at all. A listener added behind an address the emulator never advertises would still leave this step diverging.

Traces: dsql-cdc-stream-error-handling#setup#1 (representative), plus the same cluster's dsql-cluster-policy-workflow#setup#1 and dsql-vpc-endpoint-service-name#setup#1. File citations: services/service-com-amazonaws-dsql-dsql/src/state.rs lines 1622-1627 (`cluster_endpoint(region, identifier)` -> format!("{identifier}.dsql.{region}.on.aws")); services/service-com-amazonaws-dsql-dsql/src/ops/create_cluster.rs (endpoint reported in the 200 output); services/service-com-amazonaws-dsql-dsql/src/ops/get_cluster.rs (same value echoed on read).

## Evidence
Replay divergence (verdict mismatch, divergence_key service=dsql|CreateCluster|status=200->200|keys=$.endpoint), fixture dsql-cdc-stream-error-handling step setup#1, cluster of 3 steps (also dsql-cluster-policy-workflow#setup#1, dsql-vpc-endpoint-service-name#setup#1).

Expected (recorded AWS): HTTP 200 with `endpoint` present and shaped per Endpoint (^[a-zA-Z0-9.-]+$) — the literal hostname is not asserted ('<shaped-not-asserted>'), but it must be a host the responding deployment serves.

Actual (emulator): HTTP 200 with "endpoint": "01m70j916bxzay57y39x6wdey8.dsql.us-west-2.on.aws" — detail: `$.endpoint: host_not_own (expected '<shaped-not-asserted>', got '01m70j916bxzay57y39x6wdey8.dsql.us-west-2.on.aws')`. The 26-character label is the minted cluster identifier and `us-west-2` the request region, so the generated parts vary per run; the invariant part of the defect is the `.dsql.<region>.on.aws` suffix — a zone the emulator does not own and is not reached on. The emulator's data listener in this replay was :41835 on the health-checked host.

Status, error code and error message: none diverge for this step; the divergence is the value of a member in a 200 success body.

## Steps to Reproduce
1. Point an AWS DSQL client at the emulator's endpoint.
2. Call CreateCluster (POST /cluster) with deletionProtectionEnabled=false and any tags, as fixture dsql-cdc-stream-error-handling step setup#1 does.
3. Read `endpoint` from the 200 response: it is `<new-identifier>.dsql.<request-region>.on.aws`.
4. Compare that host to the host/port the emulator is being reached on (the replay's data listener): they differ — the published name is in AWS's production DNS zone, not the emulator's.
5. Attempt to connect to the published endpoint: it either fails to resolve or leaves for real AWS; it never reaches the cluster just created.
6. GetCluster(identifier=<new-identifier>) echoes the same foreign hostname.
