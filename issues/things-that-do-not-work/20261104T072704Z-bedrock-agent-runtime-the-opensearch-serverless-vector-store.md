Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime.knowledge-base-retrieval-scoring-deterministic
Issue-Key: bedrock-agent-runtime.knowledge-base-retrieval-scoring-deterministic|_create_index_when_authorized#20|RequestError|client.indices.create(index=index_name, body=body)
Issue-Commit: 2b503955bb0e5c53465d42867d341986b9868a3b

# bedrock-agent-runtime: the OpenSearch Serverless vector-store endpoint answers 400 SerializationException 'missing required header x-amz-target' to the knn index create, so no knowledge base can be populated and Retrieve scoring is unreachable

> _Note: this issue was identified against the code base at commit `2b503955bb0e5c53465d42867d341986b9868a3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Retrieve

## Description
ONE net-new root cause, grounded in a failing behavioral check listed under the bedrock-agent-runtime service token. Overlap note: the open issue 20261029T171944Z-bedrock-agent-runtime-2-net-new-causes-the.md describes this same wall as its 'Cause 1 of 2' and names this check, but the replay suite did NOT mark this check as OWNED by it (it marked only the sibling bedrock-agent-runtime.retrieve-nonexistent-knowledge-base-not-found), so it is re-filed here so the check is replayed and closed; if that issue's Cause 1 lands first, this closes with it. The same wall is also filed under the opensearchserverless and bedrock-agent__lambda__opensearchserverless tokens.

=== Cause 1 of 1: the vector store a knowledge base requires cannot be built, because the collection endpoint the check writes the knn index to serves the AWS-JSON RPC front door ===
AWS: Smithy (com.amazonaws.bedrockagentruntime) models Retrieve over a knowledge base whose vector store is an OpenSearch Serverless collection; the collection's data plane is the OpenSearch REST API and is not Smithy-modeled, so for that leg this is recorded behavior only (the control-plane model documents CollectionDetail.collectionEndpoint as the 'Collection-specific endpoint used to submit index, search, and data upload requests'). Recorded behavior: after CreateCollection(type='VECTORSEARCH') reaches ACTIVE and a data-access policy names the signing principal, a SigV4-signed (service name `aoss`) HTTP PUT of an index body to https://<collectionEndpoint>/<indexName> — exactly what opensearch-py's indices.create issues, with NO x-amz-target header, because the OpenSearch REST API has no RPC target — is accepted (200) and creates the knn_vector index; the knowledge base then ingests documents and two identical Retrieve calls return the same ranking and the same scores, with semantically relevant documents ranked first.
Emulator: that PUT is answered HTTP 400 with the body {"__type":"SerializationException","message":"missing required header `x-amz-target`"} (fixed text, nothing derived from the request), raised to the client as opensearchpy.exceptions.RequestError(400, ...). The collection endpoint host reaches the AWS-JSON `x-amz-target` dispatcher rather than an OpenSearch REST handler, so the index create — and therefore the whole knowledge base — can never be created, and Retrieve's determinism and ranking contract is never reached.
Why: the standard vector store for a Bedrock knowledge base is unusable, so every retrieval-augmented flow a client builds against this emulator stops at setup: no index, no ingestion, no Retrieve results, and the failure is reported as a serialization error on an unrelated-looking header rather than as anything actionable about the collection or the knowledge base.
Fix: the opensearchserverless collection data plane plus the emulator's host/endpoint routing table — route the host published as collectionEndpoint to an OpenSearch REST handler (index create/get/delete, document index/search) that authorises SigV4 `aoss` requests against the collection's data-access policy, instead of letting the host fall through to the AWS-JSON dispatcher; once the index can exist, the bedrock-agent-runtime Retrieve handler's scoring/ranking path becomes reachable and is what the check then measures.
Traces: bedrock-agent-runtime.knowledge-base-retrieval-scoring-deterministic.

## Evidence
Expected (recorded AWS): PUT https://<collectionEndpoint>/<indexName> signed SigV4 with service name 'aoss' (opensearch-py client.indices.create) is accepted and the knn_vector index exists; the knowledge base reaches ACTIVE, ingestion completes, and two identical Retrieve calls agree on ranking and on scores to within float32 tolerance, with the refund-policy document ranked first for a refund-policy query.

Actual (emulator), from the replay output for bedrock-agent-runtime.knowledge-base-retrieval-scoring-deterministic:
  File "http_requests.py", line 242, in perform_request
    self._raise_error(
  File "base.py", line 315, in _raise_error
    raise HTTP_EXCEPTIONS.get(status_code, TransportError)(
  opensearchpy.exceptions.RequestError: RequestError(400, '{"__type":"SerializationException","message":"missing required header `x-amz-target`"}')

The status (400), the __type (SerializationException) and the message are fixed emulator text; nothing in them derives from the request, and no Retrieve call is ever made.

## Steps to Reproduce
1. opensearchserverless.create_security_policy(type='encryption'), create_security_policy(type='network', AllowFromPublic=true), create_access_policy(type='data', granting aoss:* on collection/<name> and index/<name>/* to the calling role ARN).
2. create_collection(name=<name>, type='VECTORSEARCH'); poll batch_get_collection until status ACTIVE; read collectionDetails[0].collectionEndpoint.
3. With opensearch-py (AWSV4SignerAuth, service 'aoss') call client.indices.create(index='sim2real-kb-index', body={'settings': {'index.knn': True}, 'mappings': {'properties': {'vector_field': {'type': 'knn_vector', 'dimension': 1024, 'method': {'name': 'hnsw', 'engine': 'faiss', 'space_type': 'l2'}}, 'text_field': {'type': 'text'}, 'metadata_field': {'type': 'text', 'index': False}}}}) against https://<collectionEndpoint>.
   -> HTTP 400 {"__type":"SerializationException","message":"missing required header `x-amz-target`"} instead of a created index, so the follow-on CreateKnowledgeBase / ingestion / bedrock-agent-runtime Retrieve legs are never reached.
