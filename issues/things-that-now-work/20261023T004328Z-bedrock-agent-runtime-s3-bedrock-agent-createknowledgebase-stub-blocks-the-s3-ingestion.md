Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime__s3.retrieve-result-location-resolves-to-the-exact-ingested-s3-object
Issue-Key: bedrock-agent-runtime__s3|CreateKnowledgeBase|NotImplemented
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# bedrock-agent CreateKnowledgeBase stub blocks the S3-ingestion retrieval check: a Retrieve result's location cannot be resolved back to the ingested S3 object

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateKnowledgeBase

## Description
Cause 1 of 1: the bedrock-agent control-plane CreateKnowledgeBase operation is an unimplemented scaffold stub, so the bedrock-agent-runtime + S3 ingestion/retrieval scenario cannot be set up.
AWS: recorded behavior only. CreateKnowledgeBase returns a knowledgeBaseId; after a document is uploaded to S3 and ingested, bedrock-agent-runtime Retrieve returns retrievalResults whose location.s3Location.uri is the exact S3 URI of the ingested object, so a client can resolve a citation back to the object it came from.
Emulator: setup fails with HTTP 501 and error code NotImplemented — 'operation CreateKnowledgeBase is scaffolded but not implemented in this emulator' — so no knowledge base, no ingestion and no Retrieve result location can be observed.
Why: a client cannot stand up an S3-backed knowledge base at all, and the retrieval-provenance contract (result location pointing at the real ingested object) is entirely unverified, so a citation-rendering client has nothing to resolve.
Fix: implement CreateKnowledgeBase in services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/create_knowledge_base.rs, then the S3 data-source ingestion path that populates the Retrieve result's s3Location. (The same stub also blocks checks carrying the bedrock-agent-runtime and bedrock-agent-runtime__lambda__s3vectors service tokens; those are filed separately because they must be re-replayed under their own tokens.)
Traces: bedrock-agent-runtime__s3.retrieve-result-location-resolves-to-the-exact-ingested-s3-object.

## Evidence
expected: CreateKnowledgeBase returns a knowledgeBaseId, and a later Retrieve returns retrievalResults[*].location.s3Location.uri equal to the s3:// URI of the ingested object. actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateKnowledgeBase operation: operation CreateKnowledgeBase is scaffolded but not implemented in this emulator.

## Steps to Reproduce
Upload a document to an S3 bucket, then call bedrock-agent.create_knowledge_base(name=..., roleArn=..., knowledgeBaseConfiguration=..., storageConfiguration=...) to back it with that bucket -> the call answers 501 NotImplemented, so the subsequent Retrieve and its result-location assertion are never reached.
