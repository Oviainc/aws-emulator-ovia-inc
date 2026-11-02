Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent.start-ingestion-job-on-deleted-datasource-rejected
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent.start-ingestion-job-on-deleted-datasource-rejected|run#14|AssertionError|assert absent_code == "ResourceNotFoundException", absent_code
Issue-Commit: ca72e8d5fc2000c02a01dcf678a8fed8074b0f2e

# ListIngestionJobs answers 501 NotImplemented, so a deleted data source is never refused as not-found

> _Note: this issue was identified against the code base at commit `ca72e8d5fc2000c02a01dcf678a8fed8074b0f2e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListIngestionJobs

## Description
One net-new root cause in bedrock-agent.

Cause 1 of 1: `ListIngestionJobs` is an unimplemented scaffold stub and refuses every call with HTTP 501 `NotImplemented`, so neither the success path (enumerating a data source's ingestion jobs) nor the refusal path (a data source that no longer exists) is reachable.
Failing check ids for this cause: bedrock-agent.start-ingestion-job-on-deleted-datasource-rejected

AWS: `ListIngestionJobs` is a modeled, bound operation of this service. Smithy (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/smithy-model/com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda.json, ListIngestionJobs): required input members `knowledgeBaseId` (Id, pattern ^[0-9a-zA-Z]{10}$) and `dataSourceId` (Id, same pattern), optional `filters` (IngestionJobFilters, len 1..1), `maxResults` (MaxResults, range 1..1000), `nextToken` (NextToken, len 1..2048, pattern ^\S*$), `sortBy` (IngestionJobSortBy); required output member `ingestionJobSummaries` (IngestionJobSummaries) plus optional `nextToken`; modeled errors AccessDeniedException (403), InternalServerException (500), ResourceNotFoundException (404), ThrottlingException (429), ValidationException (400). Doc: "Lists the data ingestion jobs for a data source. The list also includes information about each job." On a `knowledgeBaseId`/`dataSourceId` pair whose data source has been deleted, AWS refuses the read with the modeled 404 `ResourceNotFoundException`; the recorded behavior for this service words that message with the fixed text `DataSource with id ` + ` is not found.` around the `dataSourceId` taken from the request — i.e. `DataSource with id <dataSourceId> is not found.`, exactly the wording the emulator already produces for `StartIngestionJob` on the same deleted data source in this same check. For a live data source AWS answers 200 with `ingestionJobSummaries` containing the jobs `StartIngestionJob` created for that data source.

Emulator: every `ListIngestionJobs` call is refused with HTTP 501 and error code `NotImplemented`, message `operation ListIngestionJobs is scaffolded but not implemented in this emulator`. The handler's `handle` body is only `Err(crate::scaffolded::Scaffolded::not_implemented("ListIngestionJobs"))`, and the file still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker. The replay shows the check reaching the right answer from `StartIngestionJob` (404 `ResourceNotFoundException`, message `DataSource with id <dataSourceId> is not found.`) and then failing on the very next call, where the asserted error code came back as `NotImplemented` instead of `ResourceNotFoundException`.

Note: the suite did not mark this check as blocked-by-an-unimplemented-operation, because the stub's refusal was consumed by the check's own error-code comparison rather than bubbling out as a ClientError, so no issue exists for this operation (list_existing_issues finds none naming ListIngestionJobs or IngestionJob). The open bedrock-agent issues (e.g. 20261029T191222Z-bedrock-agent-the-opensearch-serverless-collectionendpoint-answers-the.md, 20261030T000542Z-bedrock-agent-5-net-new-causes-vector-knowledge.md) concern the OpenSearch Serverless collection endpoint and vector knowledge-base configuration, not this operation.

Why: a client that enumerates a data source's ingestion jobs — the normal way to discover job ids and poll progress without having captured the id from the mutating call — gets a 501 for a request AWS always answers. Because `NotImplemented` is not a modeled error of this operation, SDK error handling falls through to a generic server failure rather than the retryable/terminal classes AWS declares, so the caller cannot distinguish "this data source is gone" (terminal, 404) from a server fault. The absence check that matters is lost too: after a data source is deleted the emulator never states that no ingestion jobs exist for it, so an orchestrator treating a 501 as transient retries forever against a resource that will never come back.

Fix: belongs in this operation's handler — `services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/list_ingestion_jobs.rs`. Implement `handle` to read the service state (`crate::state::service_state`), resolve the knowledge base and data source, refuse an unknown/deleted data source with the modeled `ResourceNotFoundException` carrying the message template above, and otherwise serialize the stored `crate::state::IngestionJobRecord` entries for that knowledge-base/data-source pair into `ingestionJobSummaries` (honouring `filters`, `sortBy`, `maxResults`/`nextToken` paging), then delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line at the top of the file. `StartIngestionJob` and `GetIngestionJob` in the same directory are already implemented against that record, so the state this operation must read already exists.

Traces: bedrock-agent.start-ingestion-job-on-deleted-datasource-rejected (assertion `absent_code == "ResourceNotFoundException"` observed `NotImplemented`).

## Evidence
Check: bedrock-agent.start-ingestion-job-on-deleted-datasource-rejected (verdict: mismatch)

Expected (AWS), for ListIngestionJobs on a data source that has been deleted:
  HTTP 404
  Error.Code    = ResourceNotFoundException
  Error.Message = "DataSource with id <dataSourceId> is not found."
                  fixed text: "DataSource with id " ... " is not found."
                  variable part: <dataSourceId>, taken verbatim from the request's dataSourceId

Expected (AWS), for ListIngestionJobs on a live data source:
  HTTP 200 with ingestionJobSummaries listing the jobs StartIngestionJob created for it

Actual (emulator):
  HTTP 501
  Error.Code    = NotImplemented
  Error.Message = "operation ListIngestionJobs is scaffolded but not implemented in this emulator"

Replay excerpt:
  File "check_bedrock_agent_start_ingestion_job_on_deleted_datasource_rejected.py", line 41, in run
    assert absent_code == "ResourceNotFoundException", absent_code
  AssertionError: NotImplemented

The preceding StartIngestionJob assertions in the same check PASSED (404 ResourceNotFoundException with the "DataSource with id ... is not found." message), so the divergence is isolated to ListIngestionJobs.

Source of the refusal — services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/list_ingestion_jobs.rs:
  line 4:   //! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented`
  line 180: Err(crate::scaffolded::Scaffolded::not_implemented("ListIngestionJobs"))
Siblings already implemented (no SCAFFOLDED marker): src/ops/start_ingestion_job.rs, src/ops/get_ingestion_job.rs.

## Steps to Reproduce
1. Create an IAM role trusted by bedrock.amazonaws.com, an S3 bucket, and a VECTOR knowledge base (S3_VECTORS storage) via bedrock-agent CreateKnowledgeBase; wait for the KB to leave CREATING.
2. CreateDataSource on that knowledge base with an S3 dataSourceConfiguration; note dataSourceId.
3. DeleteDataSource for that dataSourceId and poll GetDataSource until it answers 404 ResourceNotFoundException.
4. Call StartIngestionJob(knowledgeBaseId, dataSourceId) — observe the correct 404 ResourceNotFoundException with message "DataSource with id <dataSourceId> is not found.".
5. Call ListIngestionJobs(knowledgeBaseId=<kb>, dataSourceId=<deleted ds>).
   Expected: HTTP 404, ResourceNotFoundException, message "DataSource with id <dataSourceId> is not found.".
   Actual: HTTP 501, NotImplemented, "operation ListIngestionJobs is scaffolded but not implemented in this emulator".
6. For the positive side: CreateDataSource again, StartIngestionJob on the live data source, then ListIngestionJobs for it.
   Expected: HTTP 200 with that job present in ingestionJobSummaries. Actual: the same HTTP 501.
