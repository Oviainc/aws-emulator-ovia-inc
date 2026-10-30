Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-data-source-custom-connector-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-data-source-custom-connector-lifecycle bedrock-agent-knowledge-base-lifecycle
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent|GetDataSource|2xx->2xx|$.dataSource.dataSourceConfiguration.managedKnowledgeBaseConnectorConfiguration.connectorParameters
Issue-Key: bedrock-agent|GetDataSource|2xx->2xx|$.dataSource.dataSourceConfiguration.managedKnowledgeBaseConnectorConfiguration.mediaExtractionConfiguration
Issue-Key: bedrock-agent|UpdateDataSource|2xx->2xx|$.dataSource.dataSourceConfiguration.managedKnowledgeBaseConnectorConfiguration.connectorParameters
Issue-Key: bedrock-agent|UpdateDataSource|2xx->2xx|$.dataSource.dataSourceConfiguration.managedKnowledgeBaseConnectorConfiguration.mediaExtractionConfiguration
Issue-Key: bedrock-agent|UpdateDataSource|2xx->2xx|$.dataSource.status
Issue-Key: bedrock-agent|UpdateKnowledgeBase|2xx->2xx|$.knowledgeBase.status
Issue-Commit: e2a1f09908a88618f6c3959f038403dc295ad73c

# bedrock-agent read-back and update responses omit connector defaults and never report the UPDATING state

> _Note: this issue was identified against the code base at commit `e2a1f09908a88618f6c3959f038403dc295ad73c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataSource

## Description
The data-source and knowledge-base resources the emulator projects on the READ and UPDATE paths are incomplete in two independent ways: the managed-connector configuration it returns is the caller's submitted document rather than the service-completed one, and mutating calls report the resource's steady-state status instead of the transitional UPDATING status that AWS reports while the mutation is applied. 3 causes.

NOTE on related open/closed issues: the open issue 20261029T172025Z-bedrock-agent-data-source-projection-drops-connector-defaults.md covers the SAME projection defect on CreateDataSource and the CREATING state, and the resolved issue 20261029T172025Z-bedrock-agent-update-operations-return-success-but-never.md covered update PERSISTENCE (mutations are now persisted). Neither owns the three replay keys below, which is why they are filed here; a fixer should expect the create-path projection work to be shared with the first of those.

Cause 1 of 3: GetDataSource returns a truncated managed-connector configuration (fixture bedrock-agent-data-source-custom-connector-lifecycle, step task#5; same cluster also covers task#8).
AWS: Smithy: GetDataSource GET /knowledgebases/{knowledgeBaseId}/datasources/{dataSourceId} -> 200 {dataSource: DataSource (@required)}; DataSource.dataSourceConfiguration.managedKnowledgeBaseConnectorConfiguration has connectorParameters (target Document, @document) and mediaExtractionConfiguration (MediaExtractionConfiguration, holding imageExtractionConfiguration). Recorded behavior only for the default values: the service persists the connector parameters COMPLETED with its own defaults and returns them on every read — the submitted keys (type, version, aclEnabled) plus filterConfiguration.maxFileSizeInMegaBytes — and attaches mediaExtractionConfiguration.imageExtractionConfiguration.imageExtractionStatus = ENABLED. The recording also carries connectorParameters as a JSON-encoded document value (a JSON text) rather than as an inlined object, which is what makes the replay report a type mismatch in addition to the missing members.
Emulator: answers 200, but connectorParameters is the request's object echoed back with only the submitted keys, with no defaulted filterConfiguration, and mediaExtractionConfiguration is absent from the response entirely.
Why: the gap is in the persisted resource, not in a create-time echo, so a client can never read the effective connector configuration of its data source: it cannot tell what file-size filter or media extraction the knowledge base will actually apply, and any read-back/drift comparison against the resource reports a permanent difference.
Fix: the stored data-source state (the default-application step that should complete managedKnowledgeBaseConnectorConfiguration) plus the DataSource response projection/serializer that GetDataSource shares.

Cause 2 of 3: UpdateDataSource reports AVAILABLE instead of UPDATING and repeats the truncated projection (fixture bedrock-agent-data-source-custom-connector-lifecycle, step task#7).
AWS: Smithy: UpdateDataSource PUT /knowledgebases/{knowledgeBaseId}/datasources/{dataSourceId} -> 200 {dataSource (@required)}; DataSource.status is @required DataSourceStatus = [AVAILABLE, DELETING, DELETE_UNSUCCESSFUL, CREATING, UPDATING, FAILED], documented as AVAILABLE = "the data source has been created and is ready for ingestion". The update response reports the transitional status UPDATING; the data source converges to AVAILABLE (or FAILED) afterwards. The same response carries the completed connectorParameters and the defaulted mediaExtractionConfiguration described in cause 1.
Emulator: answers 200 with $.dataSource.status = AVAILABLE in the update response itself, and again returns connectorParameters without the defaulted filterConfiguration and no mediaExtractionConfiguration.
Why: a client that submits an update and then polls for UPDATING -> AVAILABLE sees a state machine with no intermediate state. It cannot distinguish "update accepted, not yet applied" from "update complete", so convergence waits return immediately and an update that would end in FAILED is unobservable.
Fix: the UpdateDataSource handler's status transition in the data-source state machine, and the same shared DataSource projection as cause 1.

Cause 3 of 3: UpdateKnowledgeBase reports ACTIVE instead of UPDATING (fixture bedrock-agent-knowledge-base-lifecycle, step task#5).
AWS: Smithy: UpdateKnowledgeBase PUT /knowledgebases/{knowledgeBaseId} -> 202 {knowledgeBase (@required)}; KnowledgeBase.status is KnowledgeBaseStatus = [CREATING, ACTIVE, DELETING, UPDATING, FAILED, DELETE_UNSUCCESSFUL, UPDATE_UNSUCCESSFUL]. The 202 (accepted-for-processing) response reports UPDATING, and the knowledge base settles to ACTIVE — or to UPDATE_UNSUCCESSFUL — afterwards.
Emulator: answers 202 with $.knowledgeBase.status = ACTIVE, i.e. the mutation is reported as already complete at accept time; UPDATING and UPDATE_UNSUCCESSFUL are never reachable.
Why: the 202 contract tells a client the update is asynchronous, but the status says otherwise, so a client that waits for UPDATING before polling for ACTIVE either never observes the transition or treats the response as inconsistent; the modeled failure state UPDATE_UNSUCCESSFUL can never be exercised.
Fix: the UpdateKnowledgeBase handler's status transition in the knowledge-base state machine.

In all three causes only the status values, the defaulted members and the document encoding are requirements; the ids, names, role ARNs, bucket names and timestamps in the recordings are per-run values.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200):

Cause 1 — GetDataSource, expected 200 / actual 200 (bedrock-agent-data-source-custom-connector-lifecycle#task#5; cluster also covers bedrock-agent-data-source-custom-connector-lifecycle#task#8):
  $.dataSource.dataSourceConfiguration.managedKnowledgeBaseConnectorConfiguration.connectorParameters: type_mismatch (expected '{"type":"CUSTOM","aclEnabled":false,"filterConfiguration":{"maxFileSizeInMegaBytes":"500"},"version":"1"}', got {'type': 'CUSTOM', 'version': '1', 'aclEnabled': False})
  $.dataSource.dataSourceConfiguration.managedKnowledgeBaseConnectorConfiguration.mediaExtractionConfiguration: missing (expected {'imageExtractionConfiguration': {'imageExtractionStatus': 'ENABLED'}}, got None)

Cause 2 — UpdateDataSource, expected 200 / actual 200 (bedrock-agent-data-source-custom-connector-lifecycle#task#7):
  $.dataSource.status: value_mismatch (expected 'UPDATING', got 'AVAILABLE')
  $.dataSource.dataSourceConfiguration.managedKnowledgeBaseConnectorConfiguration.connectorParameters: type_mismatch (expected '{"type":"CUSTOM","aclEnabled":false,"filterConfiguration":{"maxFileSizeInMegaBytes":"500"},"version":"1"}', got {'aclEnabled': False, 'version': '1', 'type': 'CUSTOM'})
  $.dataSource.dataSourceConfiguration.managedKnowledgeBaseConnectorConfiguration.mediaExtractionConfiguration: missing (expected {'imageExtractionConfiguration': {'imageExtractionStatus': 'ENABLED'}}, got None)

Cause 3 — UpdateKnowledgeBase, expected 202 / actual 202 (bedrock-agent-knowledge-base-lifecycle#task#5):
  $.knowledgeBase.status: value_mismatch (expected 'UPDATING', got 'ACTIVE')

Smithy (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda): DataSourceStatus = [AVAILABLE, DELETING, DELETE_UNSUCCESSFUL, CREATING, UPDATING, FAILED]; KnowledgeBaseStatus = [CREATING, ACTIVE, DELETING, UPDATING, FAILED, DELETE_UNSUCCESSFUL, UPDATE_UNSUCCESSFUL]; DataSource.status @required; ManagedKnowledgeBaseConnectorConfiguration.{connectorParameters: Document (@document), mediaExtractionConfiguration: MediaExtractionConfiguration}; UpdateKnowledgeBase http {method: PUT, uri: /knowledgebases/{knowledgeBaseId}, code: 202}; UpdateDataSource http {method: PUT, code: 200}; GetDataSource http {method: GET, code: 200}. No status, error code or error message diverges in these three causes — all three are 2xx responses whose body is wrong.

## Steps to Reproduce
1. Replay fixture bedrock-agent-data-source-custom-connector-lifecycle. At step task#5 (GetDataSource on the CUSTOM managed-connector data source created in setup#4) inspect $.dataSource.dataSourceConfiguration.managedKnowledgeBaseConnectorConfiguration: connectorParameters comes back as the submitted object without the service-defaulted filterConfiguration.maxFileSizeInMegaBytes, and mediaExtractionConfiguration is absent. Step task#8 shows the same.
2. Same fixture, step task#7 (UpdateDataSource with a changed description): the 200 response reports $.dataSource.status = AVAILABLE where UPDATING is expected, and repeats the truncated connectorParameters/mediaExtractionConfiguration projection.
3. Replay fixture bedrock-agent-knowledge-base-lifecycle, step task#5 (UpdateKnowledgeBase): the 202 response reports $.knowledgeBase.status = ACTIVE where UPDATING is expected; polling afterwards never shows a transitional state.
