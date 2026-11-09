Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime.knowledge-base-retrieval-scoring-deterministic bedrock-agent-runtime.retrieve-nonexistent-knowledge-base-not-found
Issue-Key: bedrock-agent-runtime.knowledge-base-retrieval-scoring-deterministic|run#14|PublishedUrlRejected|route_url, route_headers = ctx.endpoint_route(endpoint)
Issue-Key: bedrock-agent-runtime.retrieve-nonexistent-knowledge-base-not-found|run#14|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(host)
Issue-Commit: 44786d0c9f40e8c1d9dd84c42bcd4a9c51c55d07

# bedrock-agent-runtime: the AOSS vector-store endpoint a retrieval knowledge base is built on is published as the real AWS host <id>.<region>.aoss.amazonaws.com, an address the emulator does not serve, so Retrieve can never be exercised

> _Note: this issue was identified against the code base at commit `44786d0c9f40e8c1d9dd84c42bcd4a9c51c55d07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetCollection

## Description
REOPENS 20261104T072704Z-bedrock-agent-runtime-the-opensearch-serverless-vector-store.md: its fix left the call refused. That issue reported the OpenSearch Serverless vector-store endpoint answering 400 SerializationException 'missing required header x-amz-target' to the vector-index create both bedrock-agent-runtime retrieval checks must make before they can call Retrieve. After the fix the same call is still never served: the host published for the collection is now the production AWS name <collection-id>.<region>.aoss.amazonaws.com, which the emulator does not serve, so the request is refused at egress before it is sent.

ONE root cause, grounded in two failing behavioral checks listed under the suite heading `### bedrock-agent-runtime`.

=== Cause 1 of 1: the collectionEndpoint published for the retrieval knowledge base's AOSS vector store is a production AWS hostname instead of a host:port the emulator serves ===
AWS: Smithy (com.amazonaws.opensearchserverless) documents CollectionDetail.collectionEndpoint as the "Collection-specific endpoint used to submit index, search, and data upload requests to an OpenSearch Serverless collection"; the OpenSearch REST API spoken there is not Smithy-modeled, so the data plane is recorded behavior only. Recorded: a knowledge base with an OPENSEARCH_SERVERLESS storage configuration requires its vector index to exist first, so the caller reads collectionEndpoint and issues the SigV4 'aoss'-signed `PUT /<index>` with the knn_vector mapping there; on AWS that host resolves to the service and serves it. For an emulator the derived requirement is that the published value be an address the emulator answers on — the harness states the contract verbatim: "a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". The URL template is the requirement; the collection id, region label and port of a run are values that fill it.
Emulator: BatchGetCollection publishes collectionDetails[0].collectionEndpoint = https://<24-char collection id>.<region>.aoss.amazonaws.com, and the harness refuses to route to it: `simparity.published.PublishedUrlRejected: endpoint_url was given https://<id>.<region>.aoss.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts ...`. No HTTP status, error code or body is produced; nothing reaches the emulator.
Why: a client cannot build an AOSS-backed retrieval knowledge base at all, so bedrock-agent-runtime Retrieve is unusable for the vector-store path: neither the deterministic relevance scoring/ordering of retrievalResults nor the not-found refusal for a nonexistent knowledgeBaseId can be reached, and a client that follows AWS's documented RAG setup is pointed off the emulator at the index-create step.
Fix: the collection projection/serializer in the opensearchserverless service crate that fills CollectionDetail.collectionEndpoint, plus the emulator's host/listener registration for that endpoint — the published value must name a host:port the emulator listens on and routes to the collection's OpenSearch REST data plane. No change is needed in the bedrock-agent-runtime handlers.
Traces: bedrock-agent-runtime.knowledge-base-retrieval-scoring-deterministic, bedrock-agent-runtime.retrieve-nonexistent-knowledge-base-not-found.

## Evidence
Expected (AWS/emulator contract): once batch_get_collection reports the VECTORSEARCH collection ACTIVE, its collectionEndpoint is a URL the serving emulator answers on, so the aoss-signed `PUT /<index>` that creates the knowledge base's vector index is delivered and answered, and the check can go on to bedrock-agent-runtime.retrieve.

Actual (emulator): both checks fail in setup, in context.py endpoint_route -> _published_target:
- bedrock-agent-runtime.knowledge-base-retrieval-scoring-deterministic [mismatch]: `simparity.published.PublishedUrlRejected: endpoint_url was given https://59cec3gwhirp8b9pib1kyib9.us-west-2.aoss.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:44867/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`
- bedrock-agent-runtime.retrieve-nonexistent-knowledge-base-not-found [mismatch]: the same refusal, published https://pzx0u0oekbb5wjup7as5krcy.us-west-2.aoss.amazonaws.com, emulator reachable at 127.0.0.1:43203.

The collection ids and ports are per-run values; the divergence is the shape of the published host (a production *.aoss.amazonaws.com name) versus an address the emulator serves.

## Steps to Reproduce
1. Create the S3 source bucket/object and the knowledge-base IAM role.
2. opensearchserverless.create_security_policy(type='encryption'/'network') and create_access_policy(type='data', Principal=[<KB role ARN>]).
3. opensearchserverless.create_collection(type='VECTORSEARCH'); poll batch_get_collection until ACTIVE; read collectionDetails[0].collectionEndpoint.
   -> ACTUAL: https://<collection-id>.<region>.aoss.amazonaws.com — a production AWS host the emulator does not serve.
4. Create the vector index at that endpoint (aoss-signed `PUT /<index>` with the knn_vector/text/metadata mapping) before bedrock-agent.create_knowledge_base, then call bedrock-agent-runtime.retrieve.
   -> EXPECTED: the index create is delivered and answered; Retrieve then returns ordered retrievalResults with scores, and an unknown knowledgeBaseId is refused ResourceNotFoundException.
   -> ACTUAL: the published URL is refused at egress ("its host is not one of the emulator's own hosts"); nothing is sent and Retrieve is never reached.
