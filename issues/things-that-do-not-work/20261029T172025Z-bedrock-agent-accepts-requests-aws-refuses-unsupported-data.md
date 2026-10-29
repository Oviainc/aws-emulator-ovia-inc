Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-data-source-type-mismatch-errors
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-data-source-type-mismatch-errors bedrock-agent-ingestion-job-lifecycle
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent|CreateDataSource|4xx->2xx|$.message
Issue-Key: bedrock-agent|CreateDataSource|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent|CreateDataSource|4xx->2xx|status
Issue-Key: bedrock-agent|StartIngestionJob|4xx->2xx|$.message
Issue-Key: bedrock-agent|StartIngestionJob|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent|StartIngestionJob|4xx->2xx|status
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# bedrock-agent accepts requests AWS refuses: unsupported data source type and ingestion on a failed source

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataSource

## Description
Two pre-condition gates are missing: the emulator accepts a data source whose type is incompatible with the knowledge base type, and it accepts an ingestion job on a data source that is not ingestible. In both cases a modelled 4xx is replaced by a 2xx success. 2 causes.

Cause 1 of 2: CreateDataSource does not refuse a data source type unsupported by a MANAGED knowledge base (fixture bedrock-agent-data-source-type-mismatch-errors, step task#5).
AWS: Smithy: CreateDataSource models ValidationException (errorKind client, httpError 400) and DataSourceConfiguration.type is a @required DataSourceType (S3 | WEB | CONFLUENCE | SALESFORCE | SHAREPOINT | CUSTOM | REDSHIFT_METADATA | MANAGED_KNOWLEDGE_BASE_CONNECTOR), with the model noting that the self-managed configurations (s3Configuration, webConfiguration, confluenceConfiguration, sharePointConfiguration) are for self-managed knowledge bases and that managed knowledge bases must use managedKnowledgeBaseConnectorConfiguration. A data source type the knowledge base's type does not support is refused with 400 ValidationException, x-amzn-errortype: ValidationException, and the message "Unsupported data source type for MANAGED knowledge base type." — all fixed text for this error class except the knowledge base type token (MANAGED), which comes from the target knowledge base's knowledgeBaseConfiguration.type.
Emulator: answers 200 and creates the data source; no $.message and no x-amzn-errortype header.
Why: a client's invalid configuration is silently accepted, so it only discovers the incompatibility much later (at ingestion or query time, or never) instead of at the create call, and code that depends on this ValidationException to fall back to a supported connector never fires.
Fix: CreateDataSource's input validation — a cross-check of dataSourceConfiguration.type against the target knowledge base's type.

Cause 2 of 2: StartIngestionJob does not refuse a data source whose status forbids ingestion (fixture bedrock-agent-ingestion-job-lifecycle, step task#4).
AWS: Smithy: StartIngestionJob (PUT /knowledgebases/{knowledgeBaseId}/datasources/{dataSourceId}/ingestionjobs/, success 202) models ConflictException (errorKind client, httpError 409). A data source in a non-ingestible state is refused 409 ConflictException with x-amzn-errortype: ConflictException and the message "You cannot start an ingestion job on a data source with status FAILED." — fixed text "You cannot start an ingestion job on a data source with status " plus the data source's current DataSourceStatus value from state and a trailing period.
Emulator: answers 202 and starts the ingestion job; no $.message and no x-amzn-errortype header.
Why: a client that handles ConflictException by repairing the data source before retrying instead gets a job id for a job that cannot ingest anything, so the failure surfaces as an empty or silently incomplete knowledge base.
Fix: StartIngestionJob's handler pre-condition check against the data source's status in state. Note this gate is also unreachable today because the data source never leaves AVAILABLE (filed separately as the data-source state-machine defect).

## Evidence
Replay of trace set simparity-wire-aws-v1:

Cause 1 — CreateDataSource, expected 400, actual 200 (bedrock-agent-data-source-type-mismatch-errors#task#5):
  status: expected 400, got 200
  $.message: missing (expected 'Unsupported data source type for MANAGED knowledge base type.', got None)
  $headers.x-amzn-errortype: missing

Cause 2 — StartIngestionJob, expected 409, actual 202 (bedrock-agent-ingestion-job-lifecycle#task#4):
  status: expected 409, got 202
  $.message: missing (expected 'You cannot start an ingestion job on a data source with status FAILED.', got None)
  $headers.x-amzn-errortype: missing

Smithy: CreateDataSource errors include ValidationException (400) and ConflictException (409); StartIngestionJob errors include ConflictException (409) and its success code is 202. DataSourceType enum and DataSourceStatus enum as quoted in the description.

## Steps to Reproduce
1. Replay bedrock-agent-data-source-type-mismatch-errors through task#5: create a MANAGED knowledge base, then CreateDataSource with a data source type that type does not support — AWS answers 400 ValidationException with the unsupported-type message; the emulator answers 200 and creates it.
2. Replay bedrock-agent-ingestion-job-lifecycle through task#4: with the data source not in an ingestible state, call StartIngestionJob — AWS answers 409 ConflictException naming the data source's status; the emulator answers 202 with a job.
