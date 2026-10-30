Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-data-source-custom-connector-lifecycle
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-data-source-custom-connector-lifecycle bedrock-agent-flow-crud-lifecycle bedrock-agent-knowledge-base-lifecycle
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent|GetDataSource|2xx->2xx|$.dataSource.createdAt
Issue-Key: bedrock-agent|GetDataSource|2xx->2xx|$.dataSource.dataDeletionPolicy
Issue-Key: bedrock-agent|GetDataSource|2xx->2xx|$.dataSource.dataSourceConfiguration.managedKnowledgeBaseConnectorConfiguration.connectorParameters
Issue-Key: bedrock-agent|GetDataSource|2xx->2xx|$.dataSource.dataSourceConfiguration.managedKnowledgeBaseConnectorConfiguration.mediaExtractionConfiguration
Issue-Key: bedrock-agent|GetDataSource|2xx->2xx|$.dataSource.description
Issue-Key: bedrock-agent|GetDataSource|2xx->2xx|$.dataSource.updatedAt
Issue-Key: bedrock-agent|GetFlow|2xx->2xx|$.definition
Issue-Key: bedrock-agent|GetFlow|2xx->2xx|$.description
Issue-Key: bedrock-agent|GetKnowledgeBase|2xx->2xx|$.knowledgeBase.description
Issue-Related: 20261029T172025Z-bedrock-agent-serializes-createdat-updatedat-as-epoch-numbers.md
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# bedrock-agent Update operations return success but never persist the mutation

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDataSource

## Description
UpdateDataSource, UpdateFlow and UpdateKnowledgeBase are accepted, but the subsequent Get returns the resource exactly as created: the mutated members are unchanged. The shared defect is that the update handlers do not write the incoming members back into the resource store. 3 causes.

Cause 1 of 3: GetDataSource after UpdateDataSource (fixture bedrock-agent-data-source-custom-connector-lifecycle, step task#8).
AWS: Smithy: UpdateDataSource (PUT /knowledgebases/{knowledgeBaseId}/datasources/{dataSourceId}, 200) takes name (@required), description, dataSourceConfiguration (@required), dataDeletionPolicy and returns the updated DataSource; a following GetDataSource returns the new description and the new dataDeletionPolicy, and an updatedAt later than createdAt.
Emulator: GetDataSource answers 200 but $.dataSource.description is still the creation value and $.dataSource.dataDeletionPolicy is still RETAIN after the update set DELETE; updatedAt is unchanged from createdAt.
Why: a client that updates a data source and reads it back sees its own write lost — configuration changes (including the deletion policy that governs vector cleanup) silently do not take effect.
Fix: the UpdateDataSource handler's state write (and the updatedAt bump in the data-source state model).

Cause 2 of 3: GetFlow after UpdateFlow (fixture bedrock-agent-flow-crud-lifecycle, step task#5).
AWS: Smithy: UpdateFlow (PUT /flows/{flowIdentifier}/, 200) takes name (@required), description, executionRoleArn (@required) and definition, and returns the updated flow; a following GetFlow returns the new description and the stored definition (FlowDefinition, @sensitive) with its nodes and connections.
Emulator: GetFlow answers 200 but $.description is still the creation value and $.definition is absent entirely, so the definition supplied to UpdateFlow was neither stored nor returned.
Why: a flow authoring client cannot read back the definition it just wrote; PrepareFlow and flow execution then operate on an empty or stale definition.
Fix: the UpdateFlow handler's state write, including persisting FlowDefinition, and the GetFlow response projection for definition.

Cause 3 of 3: GetKnowledgeBase after UpdateKnowledgeBase (fixture bedrock-agent-knowledge-base-lifecycle, step task#6).
AWS: Smithy: UpdateKnowledgeBase (PUT /knowledgebases/{knowledgeBaseId}, 202) takes name (@required), description, roleArn (@required), knowledgeBaseConfiguration (@required) and returns the updated KnowledgeBase; a following GetKnowledgeBase returns the new description.
Emulator: GetKnowledgeBase answers 200 with $.knowledgeBase.description still at its creation value.
Why: same as cause 1 — the update silently no-ops, so a client's read-after-write is wrong.
Fix: the UpdateKnowledgeBase handler's state write.

The specific description strings and ids in the recordings are per-run values; the requirement is that the members supplied to the Update call are what the following Get returns.

## Evidence
Replay of trace set simparity-wire-aws-v1:

Cause 1 — GetDataSource, expected 200 / actual 200 (bedrock-agent-data-source-custom-connector-lifecycle#task#8), after UpdateDataSource:
  $.dataSource.description: value_mismatch (expected 'Updated custom data source description', got 'Custom data source for direct document ingestion')
  $.dataSource.dataDeletionPolicy: value_mismatch (expected 'DELETE', got 'RETAIN')
  (same step also shows the timestamp and connector-parameter defects filed separately)

Cause 2 — GetFlow, expected 200 / actual 200 (bedrock-agent-flow-crud-lifecycle#task#5), after UpdateFlow:
  $.description: value_mismatch (expected 'Updated flow description', got 'Initial flow for CRUD lifecycle test')
  $.definition: missing (expected the flow definition with nodes FlowInputNode/FlowOutputNode and connection FlowInputNodeFlowOutputNodeDataSource, got None)

Cause 3 — GetKnowledgeBase, expected 200 / actual 200 (bedrock-agent-knowledge-base-lifecycle#task#6), after UpdateKnowledgeBase:
  $.knowledgeBase.description: value_mismatch (expected 'Updated description for probe knowledge base', got 'Probe knowledge base for lifecycle testing')

Smithy: UpdateDataSource -> 200 {dataSource: DataSource}; UpdateFlow -> 200 with description + definition (FlowDefinition, sensitive) in both input and output; UpdateKnowledgeBase -> 202 {knowledgeBase: KnowledgeBase}.

## Steps to Reproduce
1. Replay bedrock-agent-data-source-custom-connector-lifecycle through task#8: UpdateDataSource with a new description and dataDeletionPolicy=DELETE, then GetDataSource — both members come back unchanged.
2. Replay bedrock-agent-flow-crud-lifecycle through task#5: UpdateFlow with a new description and a definition, then GetFlow — description is stale and definition is absent.
3. Replay bedrock-agent-knowledge-base-lifecycle through task#6: UpdateKnowledgeBase with a new description, then GetKnowledgeBase — description is stale.
