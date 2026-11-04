Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: neptunedata.create-ml-endpoint-nonexistent-training-job-rejected neptunedata.engine-status-reports-query-language-endpoints neptunedata.execute-fast-reset-perform-without-token-rejected neptunedata.get-propertygraph-statistics-after-delete-raises-not-available neptunedata.gremlin-addv-and-traverse-roundtrip neptunedata.gremlin-explain-query-returns-plan neptunedata.gremlin-malformed-query-rejected neptunedata.gremlin-profile-query-returns-timing-metrics neptunedata.gremlin-query-malformed-queryid-rejected-by-status-and-cancel neptunedata.loader-job-unknown-loadid-rejected-by-get-and-cancel neptunedata.manage-propertygraph-statistics-autocompute-toggle-persists neptunedata.opencypher-concurrent-writes-all-committed neptunedata.opencypher-create-and-match-roundtrip neptunedata.opencypher-explain-query-returns-plan neptunedata.opencypher-malformed-query-rejected neptunedata.opencypher-parameterized-query-binds-values neptunedata.opencypher-query-malformed-queryid-rejected-by-status-and-cancel neptunedata.propertygraph-summary-reflects-added-vertices neptunedata.start-loader-job-invalid-iam-role-arn-rejected
Issue-Shape-Id: com.amazonaws.neptunedata#AmazonNeptuneDataplane
Issue-Key: neptunedata.create-ml-endpoint-nonexistent-training-job-rejected|run#2|EndpointRefused|client = ctx.client("neptunedata", endpoint_url=f"{ctx.endpoint_url(endpoint)}:8182")
Issue-Key: neptunedata.engine-status-reports-query-language-endpoints|run#2|EndpointRefused|client = ctx.client("neptunedata", endpoint_url=f"{ctx.endpoint_url(endpoint)}:8182")
Issue-Key: neptunedata.execute-fast-reset-perform-without-token-rejected|run#2|EndpointRefused|client = ctx.client("neptunedata", endpoint_url=f"{ctx.endpoint_url(endpoint)}:8182")
Issue-Key: neptunedata.get-propertygraph-statistics-after-delete-raises-not-available|run#2|EndpointRefused|client = ctx.client("neptunedata", endpoint_url=f"{ctx.endpoint_url(endpoint)}:8182")
Issue-Key: neptunedata.gremlin-addv-and-traverse-roundtrip|run#2|EndpointRefused|client = ctx.client("neptunedata", endpoint_url=f"{ctx.endpoint_url(endpoint)}:8182")
Issue-Key: neptunedata.gremlin-explain-query-returns-plan|run#2|EndpointRefused|client = ctx.client("neptunedata", endpoint_url=f"{ctx.endpoint_url(endpoint)}:8182")
Issue-Key: neptunedata.gremlin-malformed-query-rejected|run#2|EndpointRefused|client = ctx.client("neptunedata", endpoint_url=f"{ctx.endpoint_url(endpoint)}:8182")
Issue-Key: neptunedata.gremlin-profile-query-returns-timing-metrics|run#2|EndpointRefused|client = ctx.client("neptunedata", endpoint_url=f"{ctx.endpoint_url(endpoint)}:8182")
Issue-Key: neptunedata.gremlin-query-malformed-queryid-rejected-by-status-and-cancel|run#2|EndpointRefused|client = ctx.client("neptunedata", endpoint_url=f"{ctx.endpoint_url(endpoint)}:8182")
Issue-Key: neptunedata.loader-job-unknown-loadid-rejected-by-get-and-cancel|run#5|EndpointRefused|client = ctx.client("neptunedata", endpoint_url=f"{ctx.endpoint_url(endpoint)}:8182")
Issue-Key: neptunedata.manage-propertygraph-statistics-autocompute-toggle-persists|run#2|EndpointRefused|client = ctx.client("neptunedata", endpoint_url=f"{ctx.endpoint_url(endpoint)}:8182")
Issue-Key: neptunedata.opencypher-concurrent-writes-all-committed|run#2|EndpointRefused|client = ctx.client("neptunedata", endpoint_url=f"{ctx.endpoint_url(endpoint)}:8182")
Issue-Key: neptunedata.opencypher-create-and-match-roundtrip|run#2|EndpointRefused|client = ctx.client("neptunedata", endpoint_url=f"{ctx.endpoint_url(endpoint)}:8182")
Issue-Key: neptunedata.opencypher-explain-query-returns-plan|run#2|EndpointRefused|client = ctx.client("neptunedata", endpoint_url=f"{ctx.endpoint_url(endpoint)}:8182")
Issue-Key: neptunedata.opencypher-malformed-query-rejected|run#2|EndpointRefused|client = ctx.client("neptunedata", endpoint_url=f"{ctx.endpoint_url(endpoint)}:8182")
Issue-Key: neptunedata.opencypher-parameterized-query-binds-values|run#2|EndpointRefused|client = ctx.client("neptunedata", endpoint_url=f"{ctx.endpoint_url(endpoint)}:8182")
Issue-Key: neptunedata.opencypher-query-malformed-queryid-rejected-by-status-and-cancel|run#2|EndpointRefused|client = ctx.client("neptunedata", endpoint_url=f"{ctx.endpoint_url(endpoint)}:8182")
Issue-Key: neptunedata.propertygraph-summary-reflects-added-vertices|run#2|EndpointRefused|client = ctx.client("neptunedata", endpoint_url=f"{ctx.endpoint_url(endpoint)}:8182")
Issue-Key: neptunedata.start-loader-job-invalid-iam-role-arn-rejected|run#2|EndpointRefused|client = ctx.client("neptunedata", endpoint_url=f"{ctx.endpoint_url(endpoint)}:8182")
Issue-Commit: e3d5599d128a91029caef4169f509524de4e0ad7

# neptunedata is entirely unreachable: the Neptune DB cluster's announced Endpoint already carries a port, so no data-plane client can be addressed at <endpoint>:8182

> _Note: this issue was identified against the code base at commit `e3d5599d128a91029caef4169f509524de4e0ad7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteOpenCypherQuery

## Description
THIS ISSUE HAS 1 CAUSE — it must be closed before submitting.

Cause 1 of 1: every neptunedata behavioral check fails before it can issue a single neptunedata request, because the DB cluster endpoint the emulator announces cannot be combined with the Neptune data-plane port. Each check creates its own Neptune cluster + instance via the neptune control plane, takes DBCluster.Endpoint from the create/describe response, and builds the data-plane client as `ctx.client("neptunedata", endpoint_url=f"{ctx.endpoint_url(endpoint)}:8182")`. Against the emulator that resolves to a URL that ALREADY contains a port, so appending the Neptune port yields a URL with two ports and the harness refuses it before any request is sent.

AWS: neptunedata has no regional endpoint — every operation is addressed at the cluster's own connection endpoint on the Neptune engine port. Smithy: com.amazonaws.neptune#DBCluster member `Endpoint` is documented as 'Specifies the connection endpoint for the primary instance of the DB cluster' — a bare DNS host name with NO scheme and NO port; the port is reported separately in the sibling member `Port` (8182 for the neptune engine). Smithy: com.amazonaws.neptunedata#AmazonNeptuneDataplane, restJson1, sigv4Name `neptune-db`, 43 operations, and its endpoint rule set resolves only host names (`https://neptune-db.{Region}.{PartitionResult#dnsSuffix}`), never a host:port pair. So the general requirement is: `Endpoint` is a host name only, and `host + ":" + Port` must be an address the Neptune data plane answers on.

Emulator: the value returned through DBCluster.Endpoint resolves to an address that already pins a port — the harness's resolved data-plane URL comes out as `https://<host>:<emulator-port>` and the check's `:8182` suffix makes it syntactically invalid. All 19 checks die identically in the harness with `simparity.target.EndpointRefused: endpoint 'https://<host>:<port>:8182' is not a http/https URL with a host` (raised from context.py `_require_emulator_url` -> target.py `validate_endpoint`). The host and port differ per run (two observed emulator ports in the same sweep), which is exactly the signature of the emulator's own listening address being published in `Endpoint` instead of a port-less cluster host name. Hypothesis: the neptune DBCluster projection writes the emulator's bound `host:port` into `Endpoint` rather than a port-less DNS-shaped cluster host that is served on the engine port 8182; neither the model nor the recording states this, so it is labelled a hypothesis — the OBSERVED facts are the refused URL above and that zero neptunedata requests reach the emulator.

Why: this is not a cosmetic string defect. Because `Endpoint` is the only way a client finds a Neptune cluster, a client doing the documented thing — read `Endpoint`, read `Port`, connect to `Endpoint:Port` — builds a malformed or unreachable address and can never talk to the graph at all. Every Gremlin and openCypher query, every loader job, every ML endpoint call, every engine-status/statistics/summary call and every fast-reset call is unusable: the emulator announces an address other than the one it is reached on. The whole 43-operation neptunedata surface is ungradable (0/19 checks), so no fidelity statement about it can be made until this is fixed.

Fix: the neptune control plane's DBCluster projection — the `Endpoint` (and, for consistency, `ReaderEndpoint` and any custom endpoint) value written to cluster state and rendered by the CreateDBCluster / DescribeDBClusters / ModifyDBCluster handlers must be a port-less host, with `Port` carrying 8182; and the neptunedata data plane must be served at `<that host>:8182` so the published address is the address the emulator answers on. Related but NOT the same defect: open issue 20261103T175309Z-neptune-db-cluster-and-cluster-snapshot-reports-omit.md covers other missing/inverted DBCluster members (AutoMinorVersionUpgrade, HostedZoneId, CustomEndpoints, ...) and does not mention `Endpoint`'s form or data-plane reachability, so it does not cover this. The same defect class is already filed for other services (20261103T032504Z-apigateway-portal-responses-return-a-host-port-url.md, 20261102T064827Z-lightsail-a-managed-relational-database-announces-a-masterendpoint-address.md), which supports treating a published host:port as a defect rather than an environment artefact.

Traces: all 19 neptunedata checks listed in trace_ids; representative examples are neptunedata.opencypher-create-and-match-roundtrip, neptunedata.gremlin-addv-and-traverse-roundtrip and neptunedata.engine-status-reports-query-language-endpoints.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout: neptunedata 0/19 passed (0%), 19 errored, 0 mismatched, 0 skipped, 0 unloadable. No check was marked ALREADY FILED or OWNED by an open issue.

All 19 failures carry the identical traceback tail:

  File "context.py", line 331, in _require_emulator_url
    validate_endpoint(url.split("?", 1)[0].split("#", 1)[0], schemes=schemes, path=True)
  File "target.py", line 155, in validate_endpoint
    raise EndpointRefused(f"endpoint {endpoint!r} is not a {'/'.join(schemes)} URL with a host")
  simparity.target.EndpointRefused: endpoint 'https://127.0.0.1:<emulator-port>:8182' is not a http/https URL with a host

Observed per-run instances of that message (the host/port are per-run values, NOT part of the requirement — the requirement is that no port be present before ':8182'):
  - neptunedata.create-ml-endpoint-nonexistent-training-job-rejected: endpoint 'https://127.0.0.1:33229:8182'
  - neptunedata.opencypher-concurrent-writes-all-committed: endpoint 'https://127.0.0.1:34537:8182'

Expected: `ctx.endpoint_url(DBCluster.Endpoint)` yields a port-less URL (in the AWS recording, `https://<cluster-id>.cluster-<resource-id>.<region>.neptune.amazonaws.com`), so `f"{...}:8182"` is a valid URL whose host serves the restJson1 neptunedata API, and each check's first neptunedata call (e.g. GetEngineStatus returning status 'healthy', ExecuteOpenCypherQuery returning results) succeeds.
Actual: the resolved URL carries the emulator's own listening port, the concatenated `:8182` produces a URL with two ports, and the harness refuses the endpoint — zero neptunedata requests are sent, so no neptunedata operation is exercised at all.

The failing checks span the full surface: openCypher (create/match roundtrip, parameterized binding, explain, malformed-query rejection, concurrent writes, query-status/cancel), Gremlin (addV/traverse, explain, profile, malformed-query rejection, query-status/cancel), loader jobs (StartLoaderJob invalid IAM role ARN, unknown loadId get/cancel), statistics/summary (propertygraph statistics after delete, autocompute toggle persistence, propertygraph summary), ML endpoints (CreateMLEndpoint with a nonexistent training job) and fast reset (perform without token).

## Steps to Reproduce
1. Clone the repo, build and boot the emulator from this checkout, and replay the simparity-behavioral-aws-v1 behavioral suite.
2. Observe neptunedata 0/19 with all 19 checks erroring in the harness before any neptunedata request is issued.
3. Reproduce by hand: create a Neptune cluster and one db.t4g.medium instance through the neptune control plane (CreateDBSubnetGroup + CreateDBCluster with Engine=neptune, EnableIAMDatabaseAuthentication=true + CreateDBInstance), then read DBCluster.Endpoint and DBCluster.Port from CreateDBCluster/DescribeDBClusters.
4. Assert Endpoint is a host name only: it must contain no ':' and no scheme, and Port must be 8182. Today Endpoint resolves to an address that already pins the emulator's port.
5. Build the data-plane URL as `https://<Endpoint>:<Port>` and send any neptunedata call (e.g. GetEngineStatus, or ExecuteOpenCypherQuery with `CREATE (n:Probe) RETURN n`). It must reach the emulator's restJson1 neptunedata front door and answer 200; today the URL is malformed/unreachable.
6. Re-replay the 19 check ids in trace_ids to confirm the fix — they must get past client construction and then be graded on their actual neptunedata assertions.
