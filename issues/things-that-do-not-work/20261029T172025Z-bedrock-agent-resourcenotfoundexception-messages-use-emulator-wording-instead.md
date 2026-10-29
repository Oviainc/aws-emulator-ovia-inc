Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-flow-error-paths
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-flow-error-paths bedrock-agent-ingestion-job-lifecycle bedrock-agent-not-found-error-paths
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent|GetFlow|4xx->4xx|$.message
Issue-Key: bedrock-agent|GetIngestionJob|4xx->4xx|$.message
Issue-Key: bedrock-agent|GetKnowledgeBase|4xx->4xx|$.message
Issue-Key: bedrock-agent|GetDataSource|4xx->4xx|$.message
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# bedrock-agent ResourceNotFoundException messages use emulator wording instead of the AWS templates

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFlow

## Description
The emulator does reach 404 ResourceNotFoundException on the not-found read paths, but the message member carries the emulator's own lowercase "could not find <thing> with identifier <id>" wording instead of the message AWS words for this error class. The error message is part of the contract, so each of these is a wire divergence even though the status and error type are right. 4 causes.

Cause 1 of 4: GetFlow on a nonexistent flow (fixture bedrock-agent-flow-error-paths, step task#3).
AWS: Smithy: GetFlow errors include ResourceNotFoundException (errorKind client, httpError 404) with a message: NonBlankString member. The flow-family wording is "Couldn't perform GetFlow operation. Failed to retrieve resource type Flow because it doesn't exist. Check the resource ID and retry your request." — fixed text throughout, with the operation name (GetFlow) and the resource type (Flow) taken from the request; the requested identifier is NOT part of this message.
Emulator: 404 with message 'could not find flow with identifier <flowIdentifier>'.
Why: a client that matches on the message to distinguish a missing flow from other 404s, or that surfaces the message to an operator, gets text that no AWS-targeting code or runbook recognises.
Fix: the GetFlow handler's not-found error construction (the flow error-message template).

Cause 2 of 4: GetIngestionJob on a nonexistent ingestion job (fixture bedrock-agent-ingestion-job-lifecycle, step task#5).
AWS: Smithy: GetIngestionJob errors include ResourceNotFoundException (404) with a message member. Wording: "IngestionJob with id <ingestionJobId> is not found." — fixed text "IngestionJob with id " and " is not found.", with the ingestionJobId taken from the request path; the knowledge base and data source ids are not part of the message.
Emulator: 404 with message 'could not find ingestion job with identifier <ingestionJobId> for data source <dataSourceId> in knowledge base <knowledgeBaseId>'.
Why: same as cause 1; this wording also leaks the enclosing resource ids that AWS does not include.
Fix: the GetIngestionJob handler's not-found error construction.

Cause 3 of 4: GetKnowledgeBase on a nonexistent knowledge base (fixture bedrock-agent-not-found-error-paths, step task#2).
AWS: ResourceNotFoundException (404) with message "KnowledgeBase with id <knowledgeBaseId> is not found." — fixed text "KnowledgeBase with id " / " is not found.", id from the request path.
Emulator: 404 with message 'could not find knowledge base with identifier <knowledgeBaseId>'.
Why: same as cause 1.
Fix: the GetKnowledgeBase handler's not-found error construction.

Cause 4 of 4: GetDataSource on a nonexistent data source (fixture bedrock-agent-not-found-error-paths, step task#6).
AWS: ResourceNotFoundException (404) with message "DataSource with id <dataSourceId> is not found." — fixed text "DataSource with id " / " is not found.", id from the request path; the knowledge base id is not part of the message.
Emulator: 404 with message 'could not find data source with identifier <dataSourceId> in knowledge base <knowledgeBaseId>'.
Why: same as cause 1.
Fix: the GetDataSource handler's not-found error construction.

These four share one root cause: the service's not-found error wording is hand-written per handler in the emulator's own voice rather than following the AWS templates above. Fixing it once in the shared not-found error helper covers all four.

## Evidence
Replay of trace set simparity-wire-aws-v1 — in every case the status (404) matched and only the message diverged:

Cause 1 — GetFlow, expected 404 / actual 404 (bedrock-agent-flow-error-paths#task#3):
  $.message: value_mismatch (expected "Couldn't perform GetFlow operation. Failed to retrieve resource type Flow because it doesn't exist. Check the resource ID and retry your request.", got 'could not find flow with identifier AAAAAAAAAA')

Cause 2 — GetIngestionJob, expected 404 / actual 404 (bedrock-agent-ingestion-job-lifecycle#task#5; cluster also covers bedrock-agent-not-found-error-paths#task#8):
  $.message: value_mismatch (expected 'IngestionJob with id NOSUCHJOB1 is not found.', got 'could not find ingestion job with identifier NOSUCHJOB1 for data source 7V8ZPnpSAX in knowledge base H8Eb9ufqOg')

Cause 3 — GetKnowledgeBase, expected 404 / actual 404 (bedrock-agent-not-found-error-paths#task#2):
  $.message: value_mismatch (expected 'KnowledgeBase with id NOSUCHKB01 is not found.', got 'could not find knowledge base with identifier NOSUCHKB01')

Cause 4 — GetDataSource, expected 404 / actual 404 (bedrock-agent-not-found-error-paths#task#6):
  $.message: value_mismatch (expected 'DataSource with id NOSUCHDS01 is not found.', got 'could not find data source with identifier NOSUCHDS01 in knowledge base NOSUCHKB01')

The literal ids (AAAAAAAAAA, NOSUCHJOB1, NOSUCHKB01, NOSUCHDS01, 7V8ZPnpSAX, H8Eb9ufqOg) are the probe's request values filling the template, not part of the required text.

Smithy: ResourceNotFoundException { message: NonBlankString } with errorKind client / httpError 404, bound to GetFlow, GetIngestionJob, GetKnowledgeBase and GetDataSource.

## Steps to Reproduce
1. Replay bedrock-agent-flow-error-paths step task#3: GetFlow with an identifier that does not exist — compare the 404 body's message against the AWS flow not-found sentence.
2. Replay bedrock-agent-ingestion-job-lifecycle step task#5: GetIngestionJob with a nonexistent ingestionJobId — expect 'IngestionJob with id <id> is not found.'
3. Replay bedrock-agent-not-found-error-paths step task#2: GetKnowledgeBase with a nonexistent id — expect 'KnowledgeBase with id <id> is not found.'
4. Replay bedrock-agent-not-found-error-paths step task#6: GetDataSource with a nonexistent id — expect 'DataSource with id <id> is not found.'
