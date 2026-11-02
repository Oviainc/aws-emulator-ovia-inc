Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-data-source-custom-connector-lifecycle
Issue-Step: teardown#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-data-source-custom-connector-lifecycle bedrock-agent-delete-knowledge-base-conflict-recovery bedrock-agent-knowledge-base-lifecycle bedrock-agent-data-source-type-mismatch-errors bedrock-agent-knowledge-base-documents-delete-workflow bedrock-agent-knowledge-base-documents-ingest-and-list bedrock-agent-flow-crud-lifecycle
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent|GetDataSource|4xx->2xx|$
Issue-Key: bedrock-agent|GetDataSource|4xx->2xx|$.message
Issue-Key: bedrock-agent|GetDataSource|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent|GetDataSource|4xx->2xx|status
Issue-Key: bedrock-agent|GetKnowledgeBase|4xx->2xx|$
Issue-Key: bedrock-agent|GetKnowledgeBase|4xx->2xx|$.message
Issue-Key: bedrock-agent|GetKnowledgeBase|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent|GetKnowledgeBase|4xx->2xx|status
Issue-Key: bedrock-agent|GetFlow|4xx->2xx|$.message
Issue-Key: bedrock-agent|GetFlow|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent|GetFlow|4xx->2xx|status
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# bedrock-agent Delete operations never remove the resource, so Get keeps answering 200

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDataSource

## Description
DeleteDataSource, DeleteKnowledgeBase and DeleteFlow are accepted but the resource is never removed from (nor marked deleted in) the emulator's state: the matching Get keeps answering 200 with the full resource indefinitely, and a client polling for the resource to disappear never terminates (replay capped the poll at 60s / 61 attempts). The shared defect is that the delete handlers do not mutate the resource store. 5 causes.

Cause 1 of 5: GetDataSource after DeleteDataSource (fixture bedrock-agent-data-source-custom-connector-lifecycle, step teardown#11).
AWS: Smithy: DeleteDataSource -> 202 with status: DataSourceStatus (DELETING is a modelled value), after which GetDataSource answers ResourceNotFoundException, httpError 404, errorKind client, with the x-amzn-errortype header naming ResourceNotFoundException and a message member. The recorded refusal message is "DataSource with id <dataSourceId> is not found." — fixed text "DataSource with id " and " is not found." with the requested dataSourceId substituted from the request path.
Emulator: GetDataSource answers 200 with the whole dataSource object (status AVAILABLE) forever; no $.message and no x-amzn-errortype header.
Why: a client cannot distinguish a deleted data source from a live one, so teardown/idempotency logic that waits for a 404 hangs, and a recreate-after-delete flow sees a stale resource.
Fix: the DeleteDataSource handler's state mutation (plus the GetDataSource not-found path in its handler).

Cause 2 of 5: GetKnowledgeBase after DeleteKnowledgeBase (fixture bedrock-agent-data-source-custom-connector-lifecycle, step teardown#13).
AWS: Smithy: DeleteKnowledgeBase -> 202 with status: KnowledgeBaseStatus; GetKnowledgeBase then answers 404 ResourceNotFoundException with x-amzn-errortype and a message. Recorded wording: "KnowledgeBase with id <knowledgeBaseId> is not found." — fixed text "KnowledgeBase with id " / " is not found.", id taken from the request path.
Emulator: 200 with the full knowledgeBase (status ACTIVE) forever; no message, no errortype header.
Why: same as cause 1; additionally a deleted knowledge base still appears usable for CreateDataSource/StartIngestionJob.
Fix: the DeleteKnowledgeBase handler's state mutation and the GetKnowledgeBase not-found path.

Cause 3 of 5: GetDataSource after DeleteDataSource, status-only assertion (fixture bedrock-agent-data-source-type-mismatch-errors, step teardown#10).
AWS: as cause 1 — GetDataSource on a deleted data source is 404 ResourceNotFoundException.
Emulator: 200 with the dataSource object (dataDeletionPolicy DELETE, status AVAILABLE) after 61 poll attempts.
Why: same as cause 1; this path also shows the DELETE data-deletion policy has no effect on removal.
Fix: same as cause 1.

Cause 4 of 5: GetKnowledgeBase after DeleteKnowledgeBase, status-only assertion (fixture bedrock-agent-data-source-type-mismatch-errors, step teardown#12).
AWS: as cause 2 — 404 ResourceNotFoundException.
Emulator: 200 with the full knowledgeBase after 61 poll attempts.
Why: same as cause 2.
Fix: same as cause 2.

Cause 5 of 5: GetFlow after DeleteFlow (fixture bedrock-agent-flow-crud-lifecycle, step teardown#8).
AWS: Smithy: DeleteFlow -> 200 with id: FlowId; GetFlow then answers 404 ResourceNotFoundException with x-amzn-errortype and a message. Recorded wording: "Couldn't perform GetFlow operation. Failed to retrieve resource type Flow because it doesn't exist. Check the resource ID and retry your request." — the whole sentence is fixed text for this error class except the operation name (GetFlow) and the resource type (Flow), which come from the request.
Emulator: 200 with the flow object; no $.message, no x-amzn-errortype.
Why: same as cause 1, for flows — delete is not observable, so a client cannot confirm teardown or safely reuse a flow name.
Fix: the DeleteFlow handler's state mutation and the GetFlow not-found path.

## Evidence
Replay of trace set simparity-wire-aws-v1:

Cause 1 — GetDataSource, expected 404, actual 200 (bedrock-agent-data-source-custom-connector-lifecycle#teardown#11):
  status: expected 404, got 200
  $.message: missing (expected 'DataSource with id dlAD5WrIZ1 is not found.', got None)
  $headers.x-amzn-errortype: missing
  $: poll: status 200 not in [404] after 61 attempt(s); body still {'dataSource': {... 'status': 'AVAILABLE', 'dataDeletionPolicy': 'RETAIN' ...}}

Cause 2 — GetKnowledgeBase, expected 404, actual 200 (bedrock-agent-data-source-custom-connector-lifecycle#teardown#13, cluster also covers bedrock-agent-delete-knowledge-base-conflict-recovery#task#7 and bedrock-agent-knowledge-base-lifecycle#teardown#9):
  status: expected 404, got 200
  $.message: missing (expected 'KnowledgeBase with id H9nyD7Ce62 is not found.')
  $headers.x-amzn-errortype: missing
  $: poll: status 200 not in [404] after 61 attempt(s); body still {'knowledgeBase': {... 'status': 'ACTIVE' ...}}

Cause 3 — GetDataSource, expected 404, actual 200 (bedrock-agent-data-source-type-mismatch-errors#teardown#10, cluster also covers bedrock-agent-knowledge-base-documents-delete-workflow#teardown#11 and bedrock-agent-knowledge-base-documents-ingest-and-list#teardown#13):
  status: expected 404, got 200; poll: status 200 not in [404] after 61 attempt(s)

Cause 4 — GetKnowledgeBase, expected 404, actual 200 (bedrock-agent-data-source-type-mismatch-errors#teardown#12, cluster also covers bedrock-agent-knowledge-base-documents-delete-workflow#teardown#13 and bedrock-agent-knowledge-base-documents-ingest-and-list#teardown#15):
  status: expected 404, got 200; poll: status 200 not in [404] after 61 attempt(s)

Cause 5 — GetFlow, expected 404, actual 200 (bedrock-agent-flow-crud-lifecycle#teardown#8):
  status: expected 404, got 200
  $.message: missing (expected "Couldn't perform GetFlow operation. Failed to retrieve resource type Flow because it doesn't exist. Check the resource ID and retry your request.")
  $headers.x-amzn-errortype: missing

Smithy: DeleteDataSource -> 202 {knowledgeBaseId, dataSourceId, status: DataSourceStatus(AVAILABLE|DELETING|DELETE_UNSUCCESSFUL|CREATING|UPDATING|FAILED)}; DeleteKnowledgeBase -> 202 {knowledgeBaseId, status: KnowledgeBaseStatus}; DeleteFlow -> 200 {id}; ResourceNotFoundException httpError 404 on GetDataSource/GetKnowledgeBase/GetFlow.

The resource ids and names in the recordings are per-run values; the requirement is the 404 ResourceNotFoundException with x-amzn-errortype and the message template above.

## Steps to Reproduce
1. Replay bedrock-agent-data-source-custom-connector-lifecycle: DeleteDataSource, then GetDataSource (teardown#11) — expect 404 ResourceNotFoundException, emulator serves 200 forever.
2. Same fixture, DeleteKnowledgeBase then GetKnowledgeBase (teardown#13).
3. Replay bedrock-agent-data-source-type-mismatch-errors teardown#10 and teardown#12 for the same two deletes.
4. Replay bedrock-agent-flow-crud-lifecycle: DeleteFlow then GetFlow (teardown#8).
5. In each case poll the Get until it 404s; the emulator never does (61 attempts, 60s cap).
