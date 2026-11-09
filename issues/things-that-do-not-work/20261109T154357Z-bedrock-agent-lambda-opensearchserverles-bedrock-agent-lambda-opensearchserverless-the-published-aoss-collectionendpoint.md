Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent__lambda__opensearchserverless.bedrock-kb-ingestion-job-writes-to-collection-positive bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-collection-arn-resolution-positive bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-data-access-policy-missing-negative bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-encryption-policy-missing-negative bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-field-mapping-positive bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-network-policy-missing-negative bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-vector-index-creation-positive bedrock-agent__lambda__opensearchserverless.deploy-bedrock-knowledge-base-vector-index-positive bedrock-agent__lambda__opensearchserverless.deploy-opensearchserverless-collection-data-access-policy-positive bedrock-agent__lambda__opensearchserverless.deploy-stack-missing-opensearchserverless-data-access-negative bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-retrieve-and-generate-positive bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-retrieve-knowledge-base-positive bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-data-plane-access-denied-negative bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-data-plane-search-positive
Issue-Key: bedrock-agent__lambda__opensearchserverless.bedrock-kb-ingestion-job-writes-to-collection-positive|_make_search_client#16|PublishedUrlRejected|url, route_headers = ctx.endpoint_route(endpoint)
Issue-Key: bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-collection-arn-resolution-positive|_make_search_client#15|PublishedUrlRejected|target = urllib.parse.urlsplit(ctx.endpoint_url(endpoint))
Issue-Key: bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-data-access-policy-missing-negative|_make_search_client#15|PublishedUrlRejected|url, route_headers = ctx.endpoint_route(endpoint)
Issue-Key: bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-encryption-policy-missing-negative|_make_search_client#21|PublishedUrlRejected|target = urllib.parse.urlsplit(ctx.endpoint_url(endpoint))
Issue-Key: bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-field-mapping-positive|_make_search_client#15|PublishedUrlRejected|url, route_headers = ctx.endpoint_route(endpoint)
Issue-Key: bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-network-policy-missing-negative|_make_search_client#14|PublishedUrlRejected|target = urllib.parse.urlsplit(ctx.endpoint_url(endpoint))
Issue-Key: bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-vector-index-creation-positive|_make_search_client#13|PublishedUrlRejected|target = urllib.parse.urlsplit(ctx.endpoint_url(endpoint))
Issue-Key: bedrock-agent__lambda__opensearchserverless.deploy-bedrock-knowledge-base-vector-index-positive|_aoss_signed_request#7|PublishedUrlRejected|url, route_headers = ctx.endpoint_route(f"{host}{path}")
Issue-Key: bedrock-agent__lambda__opensearchserverless.deploy-opensearchserverless-collection-data-access-policy-positive|_aoss_signed_request#12|PublishedUrlRejected|url, route_headers = ctx.endpoint_route(f"{host}{path}")
Issue-Key: bedrock-agent__lambda__opensearchserverless.deploy-stack-missing-opensearchserverless-data-access-negative|_aoss_signed_request#3|PublishedUrlRejected|url, route_headers = ctx.endpoint_route(f"{host}{path}")
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-retrieve-and-generate-positive|_sigv4_request#1|PublishedUrlRejected|url, route_headers = ctx.endpoint_route(url)
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-retrieve-knowledge-base-positive|_sigv4_request#1|PublishedUrlRejected|url, route_headers = ctx.endpoint_route(url)
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-data-plane-access-denied-negative|run#9|PublishedUrlRejected|url, route_headers = ctx.endpoint_route(active["collectionEndpoint"])
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-data-plane-search-positive|run#8|PublishedUrlRejected|url, route_headers = ctx.endpoint_route(active["collectionEndpoint"])
Issue-Commit: 44786d0c9f40e8c1d9dd84c42bcd4a9c51c55d07

# bedrock-agent__lambda__opensearchserverless: the published AOSS collectionEndpoint is the real AWS host <id>.<region>.aoss.amazonaws.com, an address the emulator does not serve, so every collection data-plane and knowledge-base build step is refused at egress before any request is sent

> _Note: this issue was identified against the code base at commit `44786d0c9f40e8c1d9dd84c42bcd4a9c51c55d07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetCollection

## Description
ONE root cause, grounded in fourteen failing behavioral checks listed under the suite heading `### bedrock-agent__lambda__opensearchserverless` (the OWNED/ALREADY-FILED checks of that heading — the Lambda-runtime botocore-catalog ones and the role-trust one — are excluded).

Overlap note (not a duplicate): open issues 20261029T171944Z-...-2-net-new-causes.md, 20261029T191222Z-...-4-net-new-causes.md, 20261102T091854Z-...-a-vectorsearch-collection-endpoint.md, 20261104T034157Z-...-the-published-vectorsearch-collection.md and 20261107T172551Z-...-the-published-vectorsearch-collectionendpoint.md all describe the collectionEndpoint ANSWERING the wrong thing (the AWS-JSON front door, 400 SerializationException 'missing required header x-amz-target') — i.e. a request that does reach the emulator. What fails now is strictly earlier and is different behavior: the endpoint the emulator publishes is not one of its own hosts, so no request is made at all. The suite marked none of these fourteen checks OWNED, so the new cause is filed here; if one of those issues is reworked to publish a served host, these close with it.

=== Cause 1 of 1: collectionEndpoint is serialized as the production AWS hostname instead of a host:port the emulator serves ===
AWS: Smithy (com.amazonaws.opensearchserverless) documents CollectionDetail.collectionEndpoint as the "Collection-specific endpoint used to submit index, search, and data upload requests to an OpenSearch Serverless collection"; the OpenSearch REST API spoken there is not Smithy-modeled, so the data plane is recorded behavior only. Recorded: the published host resolves to the service and serves `PUT /<index>`, `HEAD /<index>`, `PUT /<index>/_doc/<id>`, `GET /<index>/_doc/<id>`, `GET|POST /<index>/_search` and `GET /<index>/_mapping` for a SigV4 request signed with service name 'aoss' (plus x-amz-content-sha256 on body-bearing requests), and refuses an unauthorized one with 401/403 — which is why a Bedrock knowledge base can be pointed at it and why a data-access-policy denial is observable. For an emulator the derived requirement is that the published value be an address the emulator answers on; the harness states the contract verbatim: "a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". The URL template is the requirement; the collection id, the region label, the index/path segment and the port of any one run are values that fill it.
Emulator: BatchGetCollection publishes collectionDetails[0].collectionEndpoint = https://<24-char collection id>.<region>.aoss.amazonaws.com, and every consumer that routes to it (including the forms carrying a path, https://<id>.<region>.aoss.amazonaws.com/<index>) is refused at egress: `simparity.published.PublishedUrlRejected: endpoint_url was given https://<id>.<region>.aoss.amazonaws.com[/<path>] (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`. No HTTP status, error code or body is produced.
Why: the combined estate cannot be stood up at all. Creating the vector index, resolving a collection ARN into a usable endpoint, mapping knowledge-base fields, writing/searching documents from a Lambda handler, and the negative classes that depend on reaching the endpoint (missing encryption policy, missing network policy, missing data-access policy, data-plane access denied) are all gated on addressing collectionEndpoint. A client that follows AWS's documented flow is pointed off the emulator, and allow and deny become indistinguishable because neither produces a service response.
Fix: the collection projection/serializer in the opensearchserverless service crate that fills CollectionDetail.collectionEndpoint (and dashboardsEndpoint), plus the emulator's host/listener registration — the published value must name a host:port the emulator listens on and routes to the collection's OpenSearch REST data plane.
Traces: all fourteen checks listed in trace_ids; representative details are quoted in Evidence.

## Evidence
Expected (AWS/emulator contract): batch_get_collection on an ACTIVE collection returns a collectionEndpoint whose host the serving emulator answers on, so an aoss-signed REST request to it is delivered and either served (200/201) or refused by the service (401/403).

Actual (emulator): all fourteen checks fail in context.py endpoint_route -> _published_target with
`simparity.published.PublishedUrlRejected: endpoint_url was given https://<id>.us-west-2.aoss.amazonaws.com[/<path>] (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`
per-run values (published host [path] / emulator's own address):
- bedrock-kb-ingestion-job-writes-to-collection-positive: 8xfiaaa066n2hb09ayndp557 / 127.0.0.1:35259
- bedrock-kb-opensearchserverless-collection-arn-resolution-positive: tibvxuik1gcm50t7nmrm68gg / 127.0.0.1:46267
- bedrock-kb-opensearchserverless-data-access-policy-missing-negative: 6vi6bda1q7w54kjiqk2065uf / 127.0.0.1:44441
- bedrock-kb-opensearchserverless-encryption-policy-missing-negative: cuuteybm2vzz9ovtq69w8e0j / 127.0.0.1:41455
- bedrock-kb-opensearchserverless-field-mapping-positive: l3c0wup11tdft5bvk8swht8d / 127.0.0.1:35525
- bedrock-kb-opensearchserverless-network-policy-missing-negative: lbjj3x8v1rv44axo08702pr4 / 127.0.0.1:45799
- bedrock-kb-opensearchserverless-vector-index-creation-positive: xf2o3b8tmqyl7bmxadcmjthd / 127.0.0.1:37085
- deploy-bedrock-knowledge-base-vector-index-positive: 958lsyphh9ymlrze79ucagli/sim2r-idx-ed7d9d0ee91d / 127.0.0.1:34601
- deploy-opensearchserverless-collection-data-access-policy-positive: 4vrsakjck3jh0f75vu2t7uqc/sim2r-idx-eab6bbf06a6d / 127.0.0.1:36965
- deploy-stack-missing-opensearchserverless-data-access-negative: 4wi3q073w5xkeq5t4iscyjrd/sim2r-idx-8c2b5b6b01ef / 127.0.0.1:46337
- lambda-bedrock-agent-retrieve-and-generate-positive: zsjpetyb7dn73wptpxcsjz93/sim2real-vectors / 127.0.0.1:39371
- lambda-bedrock-agent-retrieve-knowledge-base-positive: rznrhj3yjzs7u62jg30losre/sim2real-vectors / 127.0.0.1:33069
- lambda-opensearchserverless-data-plane-access-denied-negative: ncroyc6f1x4pr1tgmf7bcm7b / 127.0.0.1:44673
- lambda-opensearchserverless-data-plane-search-positive: pdp5gemfyzxmm0mw47tzobcz / 127.0.0.1:39847

The collection ids, index path segments and ports are per-run values; the divergence is the shape of the published host (a production *.aoss.amazonaws.com name) versus an address the emulator serves.

## Steps to Reproduce
1. opensearchserverless.create_security_policy(type='encryption') and (type='network'); create_access_policy(type='data', Principal=[<the caller's / Lambda role ARN>]).
2. opensearchserverless.create_collection(type='VECTORSEARCH' or 'SEARCH'); poll batch_get_collection(ids=[<id>]) until ACTIVE; read collectionDetails[0].collectionEndpoint.
   -> ACTUAL: https://<collection-id>.<region>.aoss.amazonaws.com — a production AWS host the emulator does not serve.
3. Address that endpoint (directly, or with an index path appended) from the harness or from a Lambda handler to create the vector index, index a document, run a _search, or to observe a data-access denial.
   -> EXPECTED: the request is delivered to the emulator and served (200/201) or refused with a service status (401/403).
   -> ACTUAL: the published URL is refused at egress with PublishedUrlRejected ("its host is not one of the emulator's own hosts"); nothing is sent, so neither the positive paths (index create, field mapping, ingestion write, search, retrieve-and-generate) nor the negative classes (missing encryption/network/data-access policy, data-plane access denied) can be observed.
