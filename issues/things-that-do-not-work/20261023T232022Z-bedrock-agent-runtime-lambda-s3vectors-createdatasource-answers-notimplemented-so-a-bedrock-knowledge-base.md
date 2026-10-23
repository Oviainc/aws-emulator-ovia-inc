Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime__lambda__s3vectors.lambda-bedrock-retrieve-and-generate-positive
Issue-Key: bedrock-agent-runtime__lambda__s3vectors|CreateDataSource|NotImplemented
Issue-Commit: cd7a0e788c437c5439a918b59f215cef2324a883

# CreateDataSource answers NotImplemented, so a Bedrock knowledge-base retrieve-and-generate flow cannot be built

> _Note: this issue was identified against the code base at commit `cd7a0e788c437c5439a918b59f215cef2324a883`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataSource

## Description
ONE CAUSE.

Cause 1 of 1 - CreateDataSource is a scaffold stub, so no knowledge base can be given a data source and the whole RetrieveAndGenerate path is unreachable.
AWS: CreateDataSource is a modeled Bedrock Agent control-plane operation that attaches a data source (here an S3 bucket, with an S3-vectors vector store) to an existing knowledge base and answers 200 with the created dataSource (its dataSourceId and status). It is a normal, non-optional step: a knowledge base with no data source can never be ingested from, so Retrieve/RetrieveAndGenerate have nothing to read.
Emulator: HTTP error with code `NotImplemented` and message 'operation CreateDataSource is scaffolded but not implemented in this emulator'. `NotImplemented` is not a modeled error of this operation at all, so the client sees an error class the SDK's error taxonomy has no branch for.
Why: the entire retrieval-augmented-generation flow (create KB -> add data source -> ingest -> RetrieveAndGenerate through a Lambda) cannot be exercised; a client cannot even provision the resources, and the unmodeled error code defeats any error handling written against the API.
Fix: implement the CreateDataSource handler in the Bedrock Agent control-plane service crate (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda) and its data-source state.
Note: the replay did NOT mark this check as already filed, and the open issue 20261023T004328Z-bedrock-agent-runtime-retrieve-optimizeprompt-getagentmemory-deleteagentmemory-and.md covers Retrieve/OptimizePrompt/GetAgentMemory/DeleteAgentMemory/GetDocumentContent - different operations - so it does not cover CreateDataSource.
Traces: bedrock-agent-runtime__lambda__s3vectors.lambda-bedrock-retrieve-and-generate-positive.

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1, bedrock-agent-runtime__lambda__s3vectors 0/1 passed.

bedrock-agent-runtime__lambda__s3vectors.lambda-bedrock-retrieve-and-generate-positive:
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDataSource operation: operation CreateDataSource is scaffolded but not implemented in this emulator

The check had already succeeded in creating its S3 bucket, S3-vectors index and Lambda function before this call, so the emulator is up and serving; CreateDataSource is the first unimplemented operation the flow reaches.

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Re-replay check bedrock-agent-runtime__lambda__s3vectors.lambda-bedrock-retrieve-and-generate-positive.
3. The flow creates an S3 bucket with documents, an S3-vectors vector index, a Lambda function and a Bedrock knowledge base, then calls CreateDataSource to attach the bucket to the knowledge base. Expected 200 with the created dataSource (dataSourceId, status); observed an error with code 'NotImplemented' and message 'operation CreateDataSource is scaffolded but not implemented in this emulator'.
