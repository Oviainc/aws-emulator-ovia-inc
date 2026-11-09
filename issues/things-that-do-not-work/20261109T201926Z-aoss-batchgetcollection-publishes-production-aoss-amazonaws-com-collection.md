Issue-Source: trace-sweep-divergence
Issue-Trace: opensearchserverless-batch-get-collections-multi
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearchserverless-batch-get-collections-multi opensearchserverless-collection-in-group opensearchserverless-collection-lifecycle
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: aoss|BatchGetCollection|2xx->2xx|$.collectionDetails[*].collectionEndpoint
Issue-Key: aoss|BatchGetCollection|2xx->2xx|$.collectionDetails[*].dashboardEndpoint
Issue-Key: aoss|BatchGetCollection|2xx->2xx|$.collectionDetails[*].fipsEndpoints.collectionEndpoint
Issue-Key: aoss|BatchGetCollection|2xx->2xx|$.collectionDetails[*].fipsEndpoints.dashboardEndpoint
Issue-Commit: e2549a4c04e1d573e606d3750bdfbae1bb647d12

# aoss BatchGetCollection publishes production aoss.amazonaws.com collection endpoints the emulator does not serve (2 causes)

> _Note: this issue was identified against the code base at commit `e2549a4c04e1d573e606d3750bdfbae1bb647d12`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetCollection

## Description
TWO net-new divergence clusters from replaying the wire corpus simparity-wire-aws-v1 against this checkout, both on aoss BatchGetCollection and both the SAME fault: every endpoint member of CollectionDetail is serialized as the production AWS hostname instead of an address the emulator serves, so the replay flags each of them `host_not_own`. They are filed as two causes because they are two distinct divergence clusters covering a different number of collectionDetails entries and different fixtures/steps, and BOTH must be re-replayed before this issue is closed.

Overlap note (not a duplicate, and neither cluster was marked OWNED by the replay): open issue 20261109T154357Z-opensearchserverless-the-published-collectionendpoint-is-the-real-aws.md reports the same root cause from the BEHAVIORAL corpus (simparity-behavioral-aws-v1) and carries only behavioral check ids (opensearchserverless.collection-endpoint-requires-aoss-sigv4-service-name and three more) — it carries no wire replay key, so the two wire clusters below are not re-run by a fix gated on it. Open issues 20261104T072704Z-opensearchserverless-the-published-collectionendpoint-serves-the-aws-json.md and 20261108T223024Z-opensearchserverless-3-net-new-causes-on-the-collection.md describe LATER walls behind that endpoint (the AWS-JSON front door, and policy enforcement) and presuppose the endpoint can be reached at all. This issue is the wire-corpus half, and it also covers the three endpoint members those issues do not mention: dashboardEndpoint, fipsEndpoints.collectionEndpoint and fipsEndpoints.dashboardEndpoint.

=== Cause 1 of 2: a multi-collection BatchGetCollection publishes all four endpoint members of every entry as production AWS hosts ===
AWS: Smithy (com.amazonaws.opensearchserverless, shape CollectionDetail) models collectionEndpoint as the "Collection-specific endpoint used to submit index, search, and data upload requests to an OpenSearch Serverless collection", dashboardEndpoint as the "Collection-specific endpoint used to access OpenSearch Dashboards", and fipsEndpoints (FipsEndpoints{collectionEndpoint,dashboardEndpoint}) as the FIPS-140-3 pair; all four are plain String with no modeled constraint, and the protocol spoken at them (the OpenSearch REST API) is not Smithy-modeled, so the data plane is recorded behavior only. On real AWS the published values RESOLVE to the service and SERVE that API, so a client can take an endpoint straight out of BatchGetCollection and address it. The URL templates AWS uses are https://<collection-id>.<region>.aoss.amazonaws.com, the same host plus /_dashboards, and the aoss-fips host-label variants of both; the collection id, the region label and the port in any one run are per-run values that fill those templates. For an emulator the derived requirement is that the published value name a host:port the emulator itself answers on — the replay states the contract as `host_not_own`: the host of the published URL must be one of the emulator's own hosts (http(s)://127.0.0.1:<port>/<path>, or <own host>:<port> on a per-resource listener).
Emulator: BatchGetCollection answers HTTP 200 but publishes, for EVERY entry in collectionDetails, collectionEndpoint = https://<collection-id>.<region>.aoss.amazonaws.com, dashboardEndpoint = that host + /_dashboards, fipsEndpoints.collectionEndpoint = https://<collection-id>.<region>.aoss-fips.amazonaws.com and fipsEndpoints.dashboardEndpoint = that FIPS host + /_dashboards — production AWS hostnames the emulator neither serves nor resolves. The replay reports all eight members of a two-collection response (collectionDetails[0] and [1]) as `host_not_own`; the expected and actual strings are byte-identical, which is exactly the point: the recorded value is AWS's own host, and the emulator must not echo it because it is not the address it is reached on.
Why: a client that does what the API is for — read collectionEndpoint, then index/search/upload against it with SigV4 for signing name 'aoss' (opensearch-py, any OpenSearch client, curl) — is pointed off the emulator entirely. On a sandboxed estate the call dies as a DNS/connect failure with no status, no error code and no body, because no request is ever sent; on a network with egress it is aimed at real AWS. The whole collection data plane (index create/head, document index/get, _search, _mapping) and the Dashboards and FIPS variants are unreachable, and every downstream consumer that builds on a collection endpoint (e.g. a bedrock-agent knowledge base) stalls at the same wall.
Fix: the collection endpoint derivation and the collection projection in the opensearchserverless service crate — services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/state.rs collection_endpoint()/dashboard_endpoint()/fips_collection_endpoint()/fips_dashboard_endpoint() (lines 514-539, built from ARN_NAMESPACE/FIPS_ARN_NAMESPACE/DNS_SUFFIX) and CollectionRecord::detail() which copies them into CollectionDetail (around lines 965-970) — together with the emulator's host/listener registration, so the published host:port is one the emulator listens on and that listener routes to the collection data plane. CollectionRecord::publishes_host() (around line 845) is already written against the premise that a published host is one this service serves.
Traces: opensearchserverless-batch-get-collections-multi#task#6.

=== Cause 2 of 2: the same four members are published as production AWS hosts on a single-collection BatchGetCollection, across three fixtures ===
AWS: as Cause 1 — same shape, same members, same requirement that the published host be an address the emulator serves.
Emulator: identical fault on the single-entry form of the response: collectionDetails[0].collectionEndpoint, .dashboardEndpoint, .fipsEndpoints.collectionEndpoint and .fipsEndpoints.dashboardEndpoint all come back as the production https://<collection-id>.<region>.aoss(-fips).amazonaws.com hosts and are flagged `host_not_own`. This cluster is reached by three separate fixtures (batch-get-collections-multi, collection-in-group and collection-lifecycle), so it is the form every ordinary 'create a collection, then read it back' sequence hits.
Why: as Cause 1. It is listed separately because it is a separate replay cluster: fixing Cause 1 without re-replaying this step leaves the common single-collection read unverified.
Fix: same component as Cause 1 (one change closes both clusters), but both steps must be re-replayed to confirm it.
Traces: opensearchserverless-batch-get-collections-multi#task#7, and the same cluster via opensearchserverless-collection-in-group#task#4 and opensearchserverless-collection-lifecycle#task#3.

## Evidence
Both clusters come from the simparity-wire-aws-v1 replay against this checkout (emulator_up=true, 150 steps, 134 passed, score 89.3%).

Cause 1 — opensearchserverless-batch-get-collections-multi#task#6, BatchGetCollection, status 200 expected / 200 actual, verdict mismatch, divergence on eight members:
  $.collectionDetails[0].collectionEndpoint: host_not_own (expected 'https://<id>.us-west-2.aoss.amazonaws.com', got the identical string)
  $.collectionDetails[0].dashboardEndpoint: host_not_own (…aoss.amazonaws.com/_dashboards)
  $.collectionDetails[0].fipsEndpoints.collectionEndpoint: host_not_own (…aoss-fips.amazonaws.com)
  $.collectionDetails[0].fipsEndpoints.dashboardEndpoint: host_not_own (…aoss-fips.amazonaws.com/_dashboards)
  and the same four members of $.collectionDetails[1].
The two 24-character collection ids this run used (05o15z2k8ri8u1vsyqh1qdgf, 1py1kfbp17wmuh4aq5yyu0bw) and the us-west-2 label are per-run values; the divergence is the SHAPE of the published host — a production aoss.amazonaws.com / aoss-fips.amazonaws.com name — rather than a host:port the emulator serves. Expected and actual being byte-identical is the verdict's meaning: the emulator reproduced AWS's host instead of announcing its own.

Cause 2 — opensearchserverless-batch-get-collections-multi#task#7, BatchGetCollection, 200/200, the same four members of $.collectionDetails[0] flagged host_not_own; cluster members opensearchserverless-collection-in-group#task#4 and opensearchserverless-collection-lifecycle#task#3.

Source of the published value: state.rs:517 `format!("https://{collection_id}.{region}.{ARN_NAMESPACE}.{DNS_SUFFIX}")` with ARN_NAMESPACE="aoss" (state.rs:41), FIPS_ARN_NAMESPACE="aoss-fips" (:46), DNS_SUFFIX="amazonaws.com" (:49), DASHBOARDS_PATH="/_dashboards" (:52); projected into CollectionDetail by CollectionRecord::detail().

Expected (AWS/emulator contract): BatchGetCollection returns, for each requested collection, endpoint URLs whose host the emulator answers on, such that a SigV4 'aoss'-signed request to https://<collectionEndpoint>/<index> is delivered and served (or refused with a service status/error code), and likewise for the Dashboards and FIPS variants.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the aoss wire fixtures (replay_wire_corpus), or drive it with an SDK/CLI as below.
2. aoss create_security_policy(type='encryption', name=<p>, policy matching collection/<n>, AWSOwnedKey=true).
3. aoss create_collection(name=<n>, type='TIMESERIES'); repeat for a second collection <n2> to reproduce Cause 1's multi-entry form.
4. aoss batch_get_collection(ids=[<id>, <id2>]) (Cause 1) and aoss batch_get_collection(ids=[<id>]) (Cause 2); inspect collectionDetails[*].collectionEndpoint, .dashboardEndpoint, .fipsEndpoints.collectionEndpoint and .fipsEndpoints.dashboardEndpoint.
   -> EXPECTED: URLs whose host:port the emulator serves (e.g. http://127.0.0.1:<the emulator's data port>/..., or a per-resource listener host).
   -> ACTUAL: https://<collection-id>.<region>.aoss.amazonaws.com, the same + /_dashboards, and the aoss-fips variants — production AWS hosts the emulator does not serve; the replay flags all four members host_not_own.
5. Address any of those URLs with an OpenSearch client or curl (SigV4 service name 'aoss'): the request never reaches the emulator — it fails at name resolution/connect with no HTTP status at all, or leaves the estate toward real AWS.
