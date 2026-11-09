Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent.concurrent-start-ingestion-job-same-datasource bedrock-agent.data-source-lifecycle bedrock-agent.get-deleted-resource-returns-404-not-500 bedrock-agent.ingestion-job-async-completion-timing bedrock-agent.ingestion-job-fifo-per-datasource bedrock-agent.ingestion-job-lifecycle bedrock-agent.ingestion-job-timestamp-precision-preserved bedrock-agent.knowledge-base-document-blob-round-trip bedrock-agent.knowledge-base-lifecycle bedrock-agent.resource-policy-round-trip-enforced bedrock-agent.stop-ingestion-job-halts-progress bedrock-agent.stop-ingestion-job-on-terminal-job-rejected
Issue-Key: bedrock-agent.concurrent-start-ingestion-job-same-datasource|_create_vector_index#7|PublishedUrlRejected|route_url, route_headers = ctx.endpoint_route(endpoint)
Issue-Key: bedrock-agent.data-source-lifecycle|_create_vector_index#6|PublishedUrlRejected|route_url, route_headers = ctx.endpoint_route(endpoint)
Issue-Key: bedrock-agent.get-deleted-resource-returns-404-not-500|_create_vector_index#4|PublishedUrlRejected|route_url, route_headers = ctx.endpoint_route(endpoint)
Issue-Key: bedrock-agent.ingestion-job-async-completion-timing|_create_vector_index#2|PublishedUrlRejected|route_url, route_headers = ctx.endpoint_route(endpoint)
Issue-Key: bedrock-agent.ingestion-job-fifo-per-datasource|_create_vector_index#2|PublishedUrlRejected|route_url, route_headers = ctx.endpoint_route(endpoint)
Issue-Key: bedrock-agent.ingestion-job-lifecycle|_create_vector_index#9|PublishedUrlRejected|route_url, route_headers = ctx.endpoint_route(endpoint)
Issue-Key: bedrock-agent.ingestion-job-timestamp-precision-preserved|_create_vector_index#2|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(endpoint)
Issue-Key: bedrock-agent.knowledge-base-document-blob-round-trip|_create_vector_index#2|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(endpoint)
Issue-Key: bedrock-agent.knowledge-base-lifecycle|_create_vector_index#6|PublishedUrlRejected|route_url, route_headers = ctx.endpoint_route(endpoint)
Issue-Key: bedrock-agent.resource-policy-round-trip-enforced|_create_vector_index#2|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(endpoint)
Issue-Key: bedrock-agent.stop-ingestion-job-halts-progress|_create_vector_index#2|PublishedUrlRejected|route_url, route_headers = ctx.endpoint_route(endpoint)
Issue-Key: bedrock-agent.stop-ingestion-job-on-terminal-job-rejected|_create_vector_index#2|PublishedUrlRejected|route_url, route_headers = ctx.endpoint_route(endpoint)
Issue-Commit: 44786d0c9f40e8c1d9dd84c42bcd4a9c51c55d07

# bedrock-agent: the AOSS vector-store collectionEndpoint a knowledge base is built on is published as the real AWS host <id>.<region>.aoss.amazonaws.com, an address the emulator does not serve, so no bedrock-agent knowledge-base, data-source or ingestion-job check can even create its vector index

> _Note: this issue was identified against the code base at commit `44786d0c9f40e8c1d9dd84c42bcd4a9c51c55d07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetCollection

## Description
REOPENS 20261029T191222Z-bedrock-agent-the-opensearch-serverless-collectionendpoint-answers-the.md: its fix left the call refused. That issue reported the OpenSearch Serverless collectionEndpoint answering the AWS-JSON front door (400 SerializationException 'missing required header x-amz-target') to the vector-index create every bedrock-agent knowledge-base check must make first. After the fix the same call is still never served: the endpoint host published for the collection is now the production AWS name <collection-id>.<region>.aoss.amazonaws.com, which the emulator does not serve, so the request is refused at egress before it is sent and the identical set of bedrock-agent checks cannot get past their setup.

ONE root cause, grounded in twelve failing behavioral checks listed under the suite heading `### bedrock-agent`.

=== Cause 1 of 1: the collectionEndpoint published for the knowledge base's AOSS vector store is a production AWS hostname instead of a host:port the emulator serves ===
AWS: Smithy (com.amazonaws.opensearchserverless) documents CollectionDetail.collectionEndpoint as the "Collection-specific endpoint used to submit index, search, and data upload requests to an OpenSearch Serverless collection"; the OpenSearch REST API spoken there is not Smithy-modeled, so the data plane itself is recorded behavior only. Recorded: a Bedrock knowledge base backed by OPENSEARCH_SERVERLESS requires its vector index to exist in the collection first, so the caller reads collectionEndpoint and issues `PUT /<index>` (and the field-mapping/HEAD/search verbs) against it, SigV4-signed with service name 'aoss'. On AWS that host resolves to the service and serves those verbs. For an emulator the derived requirement is that the published value be an address the emulator answers on; the harness states the contract verbatim: "a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". The URL template is the requirement; the collection id, region label and port of any one run are values that fill it.
Emulator: BatchGetCollection publishes collectionDetails[0].collectionEndpoint = https://<24-char collection id>.<region>.aoss.amazonaws.com. Every bedrock-agent check that builds a knowledge base on that collection hands the value to its OpenSearch client and the call is refused before transmission: `simparity.published.PublishedUrlRejected: endpoint_url was given https://<id>.<region>.aoss.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`. No HTTP status, error code or body is produced, because no request reaches the emulator.
Why: the whole bedrock-agent knowledge-base surface is unusable end to end. A client following AWS's documented flow (create collection -> create vector index at collectionEndpoint -> CreateKnowledgeBase -> CreateDataSource -> StartIngestionJob) is pointed off the emulator at step two, so knowledge-base and data-source lifecycle, ingestion-job state machines (FIFO per data source, concurrent-start ConflictException, stop-on-terminal refusal), timestamp precision, document blob round trip and the deleted-resource 404 behavior can none of them be exercised — every one of them fails in setup for a reason that has nothing to do with the behavior under test.
Fix: the collection projection/serializer in the opensearchserverless service crate that fills CollectionDetail.collectionEndpoint, plus the emulator's host/listener registration for that endpoint — the published value must name a host:port the emulator listens on and routes to the collection's OpenSearch REST data plane. No change is needed in the bedrock-agent handlers themselves.
Traces: bedrock-agent.knowledge-base-lifecycle, bedrock-agent.data-source-lifecycle, bedrock-agent.ingestion-job-lifecycle, bedrock-agent.ingestion-job-fifo-per-datasource, bedrock-agent.concurrent-start-ingestion-job-same-datasource, bedrock-agent.ingestion-job-async-completion-timing, bedrock-agent.ingestion-job-timestamp-precision-preserved, bedrock-agent.knowledge-base-document-blob-round-trip, bedrock-agent.resource-policy-round-trip-enforced, bedrock-agent.stop-ingestion-job-halts-progress, bedrock-agent.stop-ingestion-job-on-terminal-job-rejected, bedrock-agent.get-deleted-resource-returns-404-not-500.

## Evidence
Expected (AWS/emulator contract): after batch_get_collection reports the vector collection ACTIVE, its collectionEndpoint is a URL the serving emulator answers on, so the SigV4 'aoss'-signed `PUT /<collection-name>-index` that creates the knowledge base's vector index is delivered and answered.

Actual (emulator): all twelve bedrock-agent checks fail identically in setup, in context.py endpoint_route -> _published_target, with
`simparity.published.PublishedUrlRejected: endpoint_url was given https://<id>.us-west-2.aoss.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`
with these per-run values (host published / emulator's own address):
- concurrent-start-ingestion-job-same-datasource: jqvjuap0t9w7k8vgp5yedo8s / 127.0.0.1:46065
- data-source-lifecycle: 90jb5dg02vub9mzdf56pasn2 / 127.0.0.1:35209
- get-deleted-resource-returns-404-not-500: vqsl9c6rqpozberu056d2teq / 127.0.0.1:45349
- ingestion-job-async-completion-timing: g6i7l072owqi8j1ryqq5kdn2 / 127.0.0.1:40755
- ingestion-job-fifo-per-datasource: ad02gfsxu0fkh30s6wz7rrh5 / 127.0.0.1:35461
- ingestion-job-lifecycle: 1ux41p6d216ajruy3kwv8tua / 127.0.0.1:42303
- ingestion-job-timestamp-precision-preserved: 2f7kq49zousmc9n5zwspkbma / 127.0.0.1:43043
- knowledge-base-document-blob-round-trip: rx33fg1jl29pzrbb2v2qwv0w / 127.0.0.1:36423
- knowledge-base-lifecycle: 1v8vlld9fu714i6ewq3rs3ci / 127.0.0.1:38421
- resource-policy-round-trip-enforced: ysi24byvbunab03t1xlmfe63 / 127.0.0.1:38101
- stop-ingestion-job-halts-progress: ahdy4pimp7pi38y7wij3h4kq / 127.0.0.1:41501
- stop-ingestion-job-on-terminal-job-rejected: mujqyl7k57z3pmy7f0dkij1l / 127.0.0.1:38125

The ids and ports are per-run; the divergence is the shape of the published host (a production *.aoss.amazonaws.com name) versus an address the emulator serves.

## Steps to Reproduce
1. s3.create_bucket + put_object for a document; iam.create_role for the knowledge-base role.
2. opensearchserverless.create_security_policy(type='encryption'), create_security_policy(type='network'), create_access_policy(type='data', Principal=[<the KB role ARN>]).
3. opensearchserverless.create_collection(name=<name>, type='VECTORSEARCH'); poll batch_get_collection(ids=[<id>]) until ACTIVE and read collectionDetails[0].collectionEndpoint.
   -> ACTUAL: https://<collection-id>.<region>.aoss.amazonaws.com, a production AWS host the emulator does not serve.
4. Build an OpenSearch client on that endpoint to create the vector index (`PUT /<name>-index` with the knn_vector mapping) before bedrock-agent.create_knowledge_base.
   -> EXPECTED: the index create is delivered to the emulator and answered, and the check proceeds to CreateKnowledgeBase / CreateDataSource / StartIngestionJob.
   -> ACTUAL: the published URL is refused at egress (PublishedUrlRejected: "its host is not one of the emulator's own hosts"); nothing is sent, and every bedrock-agent knowledge-base, data-source and ingestion-job behavior behind this step is unreachable.
