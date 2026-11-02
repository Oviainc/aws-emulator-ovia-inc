Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-data-source-custom-connector-lifecycle
Issue-Step: setup#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-data-source-custom-connector-lifecycle bedrock-agent-data-source-type-mismatch-errors bedrock-agent-delete-knowledge-base-conflict-recovery bedrock-agent-knowledge-base-documents-delete-workflow bedrock-agent-knowledge-base-documents-ingest-and-list bedrock-agent-ingestion-job-lifecycle
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent|CreateDataSource|2xx->2xx|$.dataSource.createdAt
Issue-Key: bedrock-agent|CreateDataSource|2xx->2xx|$.dataSource.dataSourceConfiguration.managedKnowledgeBaseConnectorConfiguration.connectorParameters
Issue-Key: bedrock-agent|CreateDataSource|2xx->2xx|$.dataSource.dataSourceConfiguration.managedKnowledgeBaseConnectorConfiguration.mediaExtractionConfiguration
Issue-Key: bedrock-agent|CreateDataSource|2xx->2xx|$.dataSource.updatedAt
Issue-Key: bedrock-agent|CreateDataSource|2xx->2xx|$.dataSource.status
Issue-Key: bedrock-agent|GetDataSource|2xx->2xx|$.dataSource.status
Issue-Related: 20261029T172025Z-bedrock-agent-update-operations-return-success-but-never.md
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# bedrock-agent data-source projection drops connector defaults and skips the CREATING state

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataSource

## Description
The DataSource the emulator stores and returns for a MANAGED_KNOWLEDGE_BASE_CONNECTOR data source is missing the service-applied connector defaults, and its lifecycle has no CREATING state — the data source is born AVAILABLE and never transitions. 4 causes.

Cause 1 of 4: CreateDataSource response projection (fixture bedrock-agent-data-source-custom-connector-lifecycle, step setup#4).
AWS: Smithy: CreateDataSource -> 200 {dataSource: DataSource}; DataSource.dataSourceConfiguration is a DataSourceConfiguration whose managedKnowledgeBaseConnectorConfiguration carries connectorParameters (target Document, @document) and mediaExtractionConfiguration (MediaExtractionConfiguration with imageExtractionConfiguration). The recording shows the service completing the caller's connector parameters with its own defaults and attaching a default media-extraction configuration: connectorParameters gains filterConfiguration.maxFileSizeInMegaBytes alongside the submitted type/version/aclEnabled, and mediaExtractionConfiguration.imageExtractionConfiguration.imageExtractionStatus is set to ENABLED (recorded behavior only — the model documents the members but not the default values).
Emulator: answers 200 but echoes connectorParameters back with only the submitted keys (type, version, aclEnabled) and omits mediaExtractionConfiguration entirely.
Why: a client reading back its data source cannot see the effective connector configuration, so it cannot tell what file-size filter or media extraction the knowledge base will actually apply, and a config-diff/drift check against the created resource always reports a difference.
Fix: the CreateDataSource handler's default-application step and the DataSource response projection for managedKnowledgeBaseConnectorConfiguration.

Cause 2 of 4: GetDataSource carries the same truncated projection (fixture bedrock-agent-data-source-custom-connector-lifecycle, step task#5).
AWS: GetDataSource -> 200 returns the same completed DataSource, including the defaulted connectorParameters and mediaExtractionConfiguration.
Emulator: 200 with connectorParameters missing the defaulted filterConfiguration and mediaExtractionConfiguration absent.
Why: the gap is not just a create-time echo — it is the persisted resource, so every read is incomplete.
Fix: same as cause 1 (the stored data-source state, which is what Get projects).

Cause 3 of 4: CreateDataSource reports AVAILABLE instead of CREATING (fixture bedrock-agent-ingestion-job-lifecycle, step setup#2).
AWS: Smithy: DataSource.status is @required DataSourceStatus with values AVAILABLE | DELETING | DELETE_UNSUCCESSFUL | CREATING | UPDATING | FAILED, documented as "Available — the data source has been created and is ready for ingestion". The create response reports CREATING; the data source becomes AVAILABLE (or FAILED) afterwards. The same step also shows the S3 connector's connectorParameters completed with filterConfiguration.maxFileSizeInMegaBytes and a default mediaExtractionConfiguration.
Emulator: the 200 create response already reports status AVAILABLE, and the connector defaults are again absent.
Why: a client that waits for CREATING -> AVAILABLE sees a state machine with no intermediate state, so it cannot distinguish "accepted" from "ready", and it can never observe a creation that ends in FAILED.
Fix: the CreateDataSource handler's initial status and the data-source state machine.

Cause 4 of 4: GetDataSource status never leaves AVAILABLE (fixture bedrock-agent-ingestion-job-lifecycle, step setup#3).
AWS: a data source whose connector configuration cannot be validated against its source converges to FAILED (DataSourceStatus FAILED is modelled, and DataSource.failureReasons exists to explain it); AVAILABLE is the wrong terminal state for that case, which is why the recorded poll treats AVAILABLE as a failing observation.
Emulator: GetDataSource reports AVAILABLE on the first poll and stays there, so no terminal failure state is ever reachable.
Why: a client cannot observe or handle a failed data source at all; downstream error paths (see the StartIngestionJob conflict gate) are unreachable.
Fix: the data-source state machine and its failure/validation transition, in the data-source state model.

## Evidence
Replay of trace set simparity-wire-aws-v1:

Cause 1 — CreateDataSource, expected 200 / actual 200 (bedrock-agent-data-source-custom-connector-lifecycle#setup#4; cluster also covers bedrock-agent-data-source-type-mismatch-errors#setup#4, bedrock-agent-delete-knowledge-base-conflict-recovery#setup#4, bedrock-agent-knowledge-base-documents-delete-workflow#setup#4, bedrock-agent-knowledge-base-documents-ingest-and-list#setup#4):
  $.dataSource.dataSourceConfiguration.managedKnowledgeBaseConnectorConfiguration.connectorParameters: type_mismatch (expected {"type":"CUSTOM","aclEnabled":false,"filterConfiguration":{"maxFileSizeInMegaBytes":"500"},"version":"1"}, got {'type': 'CUSTOM', 'version': '1', 'aclEnabled': False})
  $.dataSource.dataSourceConfiguration.managedKnowledgeBaseConnectorConfiguration.mediaExtractionConfiguration: missing (expected {'imageExtractionConfiguration': {'imageExtractionStatus': 'ENABLED'}}, got None)

Cause 2 — GetDataSource, expected 200 / actual 200 (bedrock-agent-data-source-custom-connector-lifecycle#task#5): identical connectorParameters type_mismatch and missing mediaExtractionConfiguration.

Cause 3 — CreateDataSource, expected 200 / actual 200 (bedrock-agent-ingestion-job-lifecycle#setup#2):
  $.dataSource.status: value_mismatch (expected 'CREATING', got 'AVAILABLE')
  connectorParameters: type_mismatch (expected {"type":"S3","filterConfiguration":{"inclusionPrefixes":["documents/"],"maxFileSizeInMegaBytes":"500"},"connectionConfiguration":{"bucketName":...},"aclEnabled":false,"version":"1"}, got the same object without filterConfiguration.maxFileSizeInMegaBytes and without aclEnabled)
  mediaExtractionConfiguration: missing

Cause 4 — GetDataSource, expected 200 / actual 200 (bedrock-agent-ingestion-job-lifecycle#setup#3):
  $.dataSource.status: poll: 'AVAILABLE' is in fail_on after 1 attempt(s)

Smithy: DataSourceStatus = [AVAILABLE, DELETING, DELETE_UNSUCCESSFUL, CREATING, UPDATING, FAILED]; DataSource.status @required; DataSource.failureReasons: FailureReasons; ManagedKnowledgeBaseConnectorConfiguration.{connectorParameters: Document, mediaExtractionConfiguration: MediaExtractionConfiguration}. The bucket names, prefixes and ids in the recordings are per-run values; the defaulted members and the status progression are the requirement.

## Steps to Reproduce
1. Replay bedrock-agent-data-source-custom-connector-lifecycle step setup#4 (CreateDataSource, MANAGED_KNOWLEDGE_BASE_CONNECTOR/CUSTOM) and inspect $.dataSource.dataSourceConfiguration.managedKnowledgeBaseConnectorConfiguration — connectorParameters lacks the defaulted filterConfiguration and mediaExtractionConfiguration is absent.
2. Same fixture, step task#5 (GetDataSource): the persisted resource has the same gaps.
3. Replay bedrock-agent-ingestion-job-lifecycle step setup#2 (CreateDataSource, S3 connector): $.dataSource.status is AVAILABLE where CREATING is expected.
4. Same fixture, step setup#3: poll GetDataSource — status stays AVAILABLE and no terminal FAILED state is ever reached.
