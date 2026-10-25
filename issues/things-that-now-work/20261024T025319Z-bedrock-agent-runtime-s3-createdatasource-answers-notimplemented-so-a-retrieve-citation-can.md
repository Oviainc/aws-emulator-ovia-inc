Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime__s3.retrieve-result-location-resolves-to-the-exact-ingested-s3-object
Issue-Key: bedrock-agent-runtime__s3|CreateDataSource|NotImplemented
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# CreateDataSource answers NotImplemented, so a Retrieve citation can never be resolved back to its S3 object

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataSource

## Description
ONE CAUSE.

Cause 1 of 1 - CreateDataSource is a scaffold stub, so no knowledge base can be attached to an S3 data source and the cross-service Retrieve-citation-resolves-at-S3 path is unreachable.

AWS: CreateDataSource is a modeled Bedrock Agent control-plane operation. Smithy (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda): CreateDataSource is `PUT /knowledgebases/{knowledgeBaseId}/datasources/`, @idempotent, http code 200; input requires knowledgeBaseId (@httpLabel, pattern ^[0-9a-zA-Z]{10}$), name (pattern ^([0-9a-zA-Z][_-]?){1,100}$) and dataSourceConfiguration, with optional clientToken (@idempotencyToken, length 33-256), description, dataDeletionPolicy, serverSideEncryptionConfiguration and vectorIngestionConfiguration; output requires `dataSource` (DataSource). Its ONLY modeled errors are AccessDeniedException (403), ConflictException (409), InternalServerException (500), ResourceNotFoundException (404), ServiceQuotaExceededException (402), ThrottlingException (429) and ValidationException (400). Attaching a data source is a non-optional step of the knowledge-base lifecycle: a knowledge base with no data source can never be ingested from, so Retrieve has nothing to return and no citation to point at.

Emulator: the call fails with error code `NotImplemented` and message 'operation CreateDataSource is scaffolded but not implemented in this emulator' (example message; the requirement is the status/code, not the text). `NotImplemented` is not among the operation's modeled errors, so the SDK's generated error taxonomy for CreateDataSource has no branch for it and it surfaces as a bare ClientError.

Why: a client cannot provision the resource graph at all. Everything downstream of the data source is dead - ingestion, Retrieve, and in particular the provenance claim this check exists to verify: that each retrieval result's `location.type == "S3"` and `location.s3Location.uri` names a real object that HeadObject/GetObject at S3 resolve to the exact bytes that were ingested. Because the unmodeled code is not in the operation's error union, retry/branching logic written against the API mis-handles it instead of treating it as a server fault.

Fix: implement the CreateDataSource handler and its data-source state in the Bedrock Agent control-plane service crate (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda), returning 200 with the required `dataSource` member; until then the operation should at least fail with one of its modeled errors rather than an unmodeled code.

Note on overlap: the replay did NOT mark this check as already filed or OWNED, so no open issue carries its replay key. The open issue 20261023T232022Z-bedrock-agent-runtime-lambda-s3vectors-createdatasource-answers-notimplemented-so-a-bedrock-knowledge-base.md reports the same stub but is keyed `bedrock-agent-runtime__lambda__s3vectors|CreateDataSource|NotImplemented` and carries only the trace id bedrock-agent-runtime__lambda__s3vectors.lambda-bedrock-retrieve-and-generate-positive; it does not cover this service token or this check, which exercises a different consumer (the terminal read at S3). Fixing the handler should close both.

Traces: bedrock-agent-runtime__s3.retrieve-result-location-resolves-to-the-exact-ingested-s3-object.

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1: bedrock-agent-runtime__s3 0/1 passed (1 failed). Overall 56/62.

[error] bedrock-agent-runtime__s3.retrieve-result-location-resolves-to-the-exact-ingested-s3-object
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDataSource operation: operation CreateDataSource is scaffolded but not implemented in this emulator

Expected (per Smithy): HTTP 200 with a response whose required `dataSource` member carries the created data source (its dataSourceId and status), or, on a real failure, one of the operation's modeled errors (ValidationException 400, ConflictException 409, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ThrottlingException 429, AccessDeniedException 403, InternalServerException 500).
Actual: an error with the unmodeled code `NotImplemented`.

The emulator is up and serving: the same check had already created its S3 bucket, uploaded both documents and stood up the knowledge base before reaching this call, and the sibling service bedrock-agent-runtime passes 56/56. CreateDataSource is the first unimplemented operation the flow reaches, so no assertion in the check's own hypothesis set (H1-H7) was ever evaluated.

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Re-replay check bedrock-agent-runtime__s3.retrieve-result-location-resolves-to-the-exact-ingested-s3-object.
3. The check creates an S3 bucket, uploads two topically disjoint text documents (recording each one's MD5), creates an S3-Vectors vector bucket/index and an IAM role, and creates a Bedrock knowledge base over it.
4. It then calls CreateDataSource (PUT /knowledgebases/{knowledgeBaseId}/datasources/) to attach the S3 bucket to that knowledge base. Expected HTTP 200 with the required `dataSource` member; observed an error with code `NotImplemented` and message 'operation CreateDataSource is scaffolded but not implemented in this emulator'.
5. Consequently the later legs never run: Retrieve is never called, so `location.type == "S3"`, the `s3://<bucket>/<key>` form of `location.s3Location.uri`, and the HeadObject/GetObject resolution of that URI back to the uploaded bytes are all unverified.
