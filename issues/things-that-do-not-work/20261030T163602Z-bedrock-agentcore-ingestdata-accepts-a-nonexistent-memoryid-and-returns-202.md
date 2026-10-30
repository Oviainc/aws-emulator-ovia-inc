Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-memory-extraction-ingest-not-found-errors
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-memory-extraction-ingest-not-found-errors
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore|IngestData|4xx->2xx|$.message
Issue-Key: bedrock-agentcore|IngestData|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore|IngestData|4xx->2xx|status
Issue-Commit: a3cb0c0a256cee4af1ed0d2f0cbe02cb253be47b

# IngestData accepts a nonexistent memoryId and returns 202 instead of ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `a3cb0c0a256cee4af1ed0d2f0cbe02cb253be47b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
IngestData

## Description
One net-new cause: the memory data-plane ingest operation never resolves the memory named in its request path, so a request against a memory that was never created (or has been deleted) is accepted and acknowledged as queued instead of failing with ResourceNotFoundException. The service is bootstrapped and answers this route, so this is a missing existence pre-condition in the handler, not an unimplemented operation.

Cause 1 of 1: IngestData succeeds on a memoryId that does not exist.
AWS: Smithy: IngestData POST /memories/{memoryId}/ingest with @http code 202; memoryId is an httpLabel, required, @length min 12, @pattern ^(arn:(aws|aws-cn|aws-us-gov):bedrock-agentcore:[a-z0-9-]+:[0-9]{12}:memory/)?[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$; modeled errors include ResourceNotFoundException @httpError 404 (shape doc: "The exception that occurs when the specified resource does not exist. This can happen when using an invalid identifier or when trying to access a resource that has been deleted."). When the path-addressed memory does not exist, the recorded response is HTTP 404 with header x-amzn-errortype 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock.agentcore.common/' and body $.message of the form 'Memory not found: <memoryId>' — the fixed text is 'Memory not found: ' and the only variable part is the memoryId httpLabel copied verbatim from the request URI (the recording's 'sim2real-probe-fakememory-abcdefghij' is that run's value filling the template, not part of it). The error-type header value is invariant.
Emulator: HTTP 202 with an IngestDataOutput (a sessionId) — no $.message and no x-amzn-errortype header. The request is syntactically valid, so the handler accepts the ingest and implicitly conjures the parent memory rather than resolving it first.
Why: a client that typos a memory id, or ingests into a memory that was deleted, is told the content was accepted (202 + sessionId) while on AWS the call fails with 404 ResourceNotFoundException. Content appears durably ingested but is attached to a memory that does not exist, the "create the memory first" ordering constraint is never surfaced, and the delete-then-ingest path is masked entirely — so code written against the emulator breaks at runtime on AWS.
Fix: IngestData's handler validation — resolve memoryId (bare id or memory ARN form) in the Memory data-plane state before allocating a session or queuing extraction work, and raise ResourceNotFoundException(404, x-amzn-errortype 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock.agentcore.common/') with message 'Memory not found: <memoryId>' when it misses.
Traces: bedrock-agentcore-memory-extraction-ingest-not-found-errors#task#3.

Note: the open issue 20261029T170914Z-bedrock-agentcore-missing-existence-pre-condition-startcodeinterpretersession-and-createevent-succeed.md describes the same class of missing pre-condition for StartCodeInterpreterSession and CreateEvent, and the open issue 20261029T170914Z-bedrock-agentcore-resourcenotfoundexception-message-text-and-resolution-order-wrong-across.md covers the shared ResourceNotFoundException message builder; neither covers IngestData, and this replay step is owned by no open issue.

## Evidence
Fixture bedrock-agentcore-memory-extraction-ingest-not-found-errors, step task#3, operation IngestData (cluster size 1): status expected 404, got 202; $.message missing — expected a 'Memory not found: <memoryId>' message, got None; $headers.x-amzn-errortype missing — expected 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock.agentcore.common/', got None. Divergence key: service=bedrock-agentcore|IngestData|status=404->202|keys=$.message,$headers.x-amzn-errortype,status. In the expected message the fixed text is 'Memory not found: ' and the variable part is the memoryId taken from the request path; the error-type header is invariant.

## Steps to Reproduce
1. Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator.
2. Fixture bedrock-agentcore-memory-extraction-ingest-not-found-errors, step task#3 — POST /memories/<memoryId-never-created>/ingest with a valid body (source, contentTimestamp, actorId).
3. Observe the emulator answers HTTP 202 with a sessionId; AWS answers HTTP 404 with x-amzn-errortype 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock.agentcore.common/' and $.message 'Memory not found: <memoryId>'.
