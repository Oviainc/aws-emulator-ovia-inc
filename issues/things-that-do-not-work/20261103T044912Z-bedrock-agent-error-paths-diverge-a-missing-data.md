Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-data-source-type-mismatch-errors
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-data-source-type-mismatch-errors bedrock-agent-kbdeps-in-place-embedding-change-refusals bedrock-agent-not-found-error-paths
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent|IngestKnowledgeBaseDocuments|4xx->2xx|$.message
Issue-Key: bedrock-agent|IngestKnowledgeBaseDocuments|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent|IngestKnowledgeBaseDocuments|4xx->2xx|status
Issue-Key: bedrock-agent|UpdateKnowledgeBase|4xx->4xx|$.message
Issue-Key: bedrock-agent|DeleteDataSource|4xx->4xx|$.message
Issue-Commit: 19755eee811c5d924ea257c82351bf2373edae9f

# bedrock-agent error paths diverge: a missing data-source-type refusal and two reworded exception messages

> _Note: this issue was identified against the code base at commit `19755eee811c5d924ea257c82351bf2373edae9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
IngestKnowledgeBaseDocuments

## Description
Three net-new bedrock-agent causes share one fault: the service's client-error paths are not faithful. One refusal is missing entirely (the request is accepted), and two refusals carry emulator-authored prose where AWS has a fixed sentence or names a different resource. Three causes, all on bedrock-agent, all must be closed.

Cause 1 of 3 - IngestKnowledgeBaseDocuments does not refuse a dataSourceType that disagrees with the target data source.
AWS: Smithy (com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda): IngestKnowledgeBaseDocuments PUT /knowledgebases/{knowledgeBaseId}/datasources/{dataSourceId}/documents, success @http code 202, modeled errors include ValidationException (client, httpError 400). When the dataSourceType carried in the request's documents does not match the type of the data source addressed by the path, AWS refuses the whole call with 400 ValidationException and does not ingest. Recorded response body message: "The dataSourceType that you specified in S3 doesn't match the type of the data source specified in the request header. Check that both types match and retry your request." Fixed text of that message: "The dataSourceType that you specified in " <the dataSourceType named in the request> " doesn't match the type of the data source specified in the request header. Check that both types match and retry your request." - the only variable part is the type token taken from the request (S3 in this recording); the rest is AWS's fixed wording. The response also carries the x-amzn-errortype header for ValidationException.
Emulator: answers 202 with the success shape - no $.message, no x-amzn-errortype header - i.e. it accepts the mismatched ingest instead of refusing it.
Why: a client that relies on AWS to reject a type-mismatched ingestion gets a 202 and believes the documents are being ingested into a data source that cannot hold them. The error-handling branch of any SDK caller is never exercised, and the emulator's state diverges from a real account's.
Fix: the IngestKnowledgeBaseDocuments handler's input validation - cross-check each document's dataSourceType against the stored data source's type before accepting, and emit ValidationException (400) with AWS's sentence.
Traces: bedrock-agent-data-source-type-mismatch-errors#task#6 (expected 400, got 202).

Cause 2 of 3 - UpdateKnowledgeBase's missing-storageConfiguration ValidationException is reworded.
AWS: Smithy: UpdateKnowledgeBase PUT /knowledgebases/{knowledgeBaseId}, required members name/roleArn/knowledgeBaseConfiguration, storageConfiguration optional in the model; ValidationException (client, httpError 400). When knowledgeBaseConfiguration.type is VECTOR and storageConfiguration is absent, AWS refuses with 400 ValidationException and the message "You must provide a storage configuration if you create a VECTOR knowledge base." That sentence is entirely fixed text - it has no variable parts.
Emulator: returns the right status (400) but the message "storageConfiguration is required for a knowledgeBaseConfiguration.type of VECTOR", which is emulator wording naming model member paths.
Why: clients and test suites that assert on or surface the service's refusal text see a sentence AWS never emits; message-matching error handling and golden-output comparisons break against the emulator even though the status is right.
Fix: the UpdateKnowledgeBase validation path's message catalogue - use AWS's fixed sentence for the VECTOR-without-storage-configuration refusal. Related but NOT covering this cluster: open issue 20261030T000542Z-bedrock-agent-5-net-new-causes-vector-knowledge mentions reworded VECTOR knowledge-base refusals; this replay key is unowned, so it is filed here.
Traces: bedrock-agent-kbdeps-in-place-embedding-change-refusals#task#4 (cluster also covers task#5 and task#6 of the same fixture).

Cause 3 of 3 - DeleteDataSource's ResourceNotFoundException names the knowledge base instead of the data source.
AWS: Smithy: DeleteDataSource DELETE /knowledgebases/{knowledgeBaseId}/datasources/{dataSourceId}, both ids required @httpLabel with pattern ^[0-9a-zA-Z]{10}$; ResourceNotFoundException (client, httpError 404). When the addressed data source cannot be resolved, AWS reports the DATA SOURCE as the missing resource and names both ids: "DataSource with id <dataSourceId> in knowledgeBase with id <knowledgeBaseId> is not found." Fixed text: "DataSource with id ", " in knowledgeBase with id ", " is not found."; the two ids are values taken from the request path (NOSUCHDS01 / NOSUCHKB01 in this recording).
Emulator: answers 404 but with the knowledge-base template "KnowledgeBase with id <knowledgeBaseId> is not found." - the wrong resource type and a message that drops the dataSourceId entirely.
Why: a client cannot tell which identifier was wrong. Code that parses the message, or a user debugging a delete, is pointed at the knowledge base when AWS reports the data source, so the failure is mis-diagnosed.
Fix: the DeleteDataSource handler's not-found resolution - report the data source (with both ids in AWS's sentence) rather than short-circuiting on the knowledge-base lookup's error. Note: the open stub issue 20261030T000542Z-bedrock-agent-deletedatasource-stub is stale with respect to this path - the operation now answers 404 and the divergence is the message, not the implementation.
Traces: bedrock-agent-not-found-error-paths#task#7.

## Evidence
Replay of simparity-wire-aws-v1 (emulator_up=true, 195 steps, 120 passed, score 61.5%).

Cause 1 - fixture bedrock-agent-data-source-type-mismatch-errors step task#6, IngestKnowledgeBaseDocuments:
  expected status 400, actual 202
  $.message: missing - expected "The dataSourceType that you specified in S3 doesn't match the type of the data source specified in the request header. Check that both types match and retry your request.", got None
  $headers.x-amzn-errortype: missing (expected a shaped ValidationException errortype), got None

Cause 2 - fixture bedrock-agent-kbdeps-in-place-embedding-change-refusals steps task#4 (representative), task#5, task#6, UpdateKnowledgeBase:
  status 400 == 400
  $.message: value_mismatch - expected 'You must provide a storage configuration if you create a VECTOR knowledge base.', got 'storageConfiguration is required for a knowledgeBaseConfiguration.type of VECTOR'

Cause 3 - fixture bedrock-agent-not-found-error-paths step task#7, DeleteDataSource:
  status 404 == 404
  $.message: value_mismatch - expected 'DataSource with id NOSUCHDS01 in knowledgeBase with id NOSUCHKB01 is not found.', got 'KnowledgeBase with id NOSUCHKB01 is not found.'

Model evidence (bedrock-agent Smithy model): IngestKnowledgeBaseDocuments success code 202 with ValidationException httpError 400; UpdateKnowledgeBase http 202 with ValidationException httpError 400 and optional storageConfiguration; DeleteDataSource http 202 with ResourceNotFoundException httpError 404 and required knowledgeBaseId + dataSourceId path labels.

## Steps to Reproduce
Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus, or drive the three steps directly against the bedrock-agent front door:

Cause 1: create a knowledge base and a data source of one type (e.g. a MANAGED/custom data source), then PUT /knowledgebases/{kbId}/datasources/{dsId}/documents with documents whose dataSourceType is S3. Replay fixture bedrock-agent-data-source-type-mismatch-errors step task#6. Expect 400 ValidationException with the dataSourceType-mismatch sentence; observe 202 with documentDetails.

Cause 2: PUT /knowledgebases/{kbId} (UpdateKnowledgeBase) with knowledgeBaseConfiguration.type = VECTOR and no storageConfiguration member. Replay fixture bedrock-agent-kbdeps-in-place-embedding-change-refusals step task#4 (also task#5, task#6). Expect 400 with message 'You must provide a storage configuration if you create a VECTOR knowledge base.'; observe the emulator's reworded message.

Cause 3: DELETE /knowledgebases/NOSUCHKB01/datasources/NOSUCHDS01 (DeleteDataSource) with neither id existing. Replay fixture bedrock-agent-not-found-error-paths step task#7. Expect 404 ResourceNotFoundException naming the DataSource and both ids; observe the KnowledgeBase-only message.
