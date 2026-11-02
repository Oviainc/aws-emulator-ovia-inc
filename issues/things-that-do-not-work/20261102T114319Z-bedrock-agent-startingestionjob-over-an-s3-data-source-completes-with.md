Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent.e2e-knowledge-base-ingestion-to-query-ready
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent.e2e-knowledge-base-ingestion-to-query-ready|run#26|AssertionError|assert final_job["statistics"]["numberOfDocumentsScanned"] >= 1, (
Issue-Commit: a4ef059b31cd49fc5529baf331c68065b4514ad5

# StartIngestionJob over an S3 data source completes with all-zero IngestionJobStatistics, so a crawl that scanned documents is indistinguishable from one that scanned nothing

> _Note: this issue was identified against the code base at commit `a4ef059b31cd49fc5529baf331c68065b4514ad5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartIngestionJob

## Description
ONE root cause. Cause 1 of 1: an ingestion job started for an S3 (crawl-based) data source of a knowledge base reaches its terminal COMPLETE status, but every member of its IngestionJobStatistics comes back 0 — including numberOfDocumentsScanned — even though the data source's bucket holds a source object written before the job was started. The emulator never crawls the bound data source and never accounts for the work in the job's statistics.

AWS: Smithy (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/smithy-model/com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda.json) models IngestionJobStatistics on the ingestion job returned by StartIngestionJob / GetIngestionJob / ListIngestionJobs. Its members are all PrimitiveLong with default 0 and are documented as the service's own accounting of the run: numberOfDocumentsScanned = "The total number of source documents that were scanned. Includes new, updated, and unchanged documents.", numberOfNewDocumentsIndexed = "The number of new source documents in the data source that were successfully indexed.", plus numberOfModifiedDocumentsIndexed, numberOfMetadataDocumentsScanned, numberOfMetadataDocumentsModified, numberOfDocumentsDeleted, numberOfDocumentsFailed and numberOfDocumentsSkipped. A job that crawls a data source holding N previously-unseen source documents and reaches COMPLETE therefore reports numberOfDocumentsScanned >= N and accounts for those N documents across the indexed/failed/skipped counters; the crawled documents then appear in ListKnowledgeBaseDocuments for that data source with status INDEXED. Recorded behavior only for the exact split between the counters; the requirement exercised here is that a COMPLETE crawl of a non-empty S3 data source does not report 0 scanned with 0 failed and 0 skipped.
Emulator: StartIngestionJob is accepted, the job advances to COMPLETE, and its statistics are returned as the modeled all-zero defaults: {'numberOfDocumentsScanned': 0, 'numberOfMetadataDocumentsScanned': 0, 'numberOfNewDocumentsIndexed': 0, 'numberOfModifiedDocumentsIndexed': 0, 'numberOfMetadataDocumentsModified': 0, 'numberOfDocumentsDeleted': 0, 'numberOfDocumentsFailed': 0, 'numberOfDocumentsSkipped': 0} — for a data source whose S3 bucket holds one object put immediately before the job started. Nothing is reported scanned, indexed, failed or skipped, so the object is unaccounted for in every counter. (The push path in the same check works: IngestKnowledgeBaseDocuments on a CUSTOM data source does reach INDEXED; it is the crawl path's accounting that is absent.)
Why: numberOfDocumentsScanned / numberOfNewDocumentsIndexed are the only signals a client has to decide whether a knowledge-base sync did any work. They gate "retry the sync", "alert that the bucket or prefix was empty or mis-configured", "proceed to query the knowledge base", and every ingestion dashboard. Against the emulator a successful crawl of a populated bucket is byte-identical to a crawl of an empty one, so a genuinely empty/mis-prefixed data source is never caught and no assertion that documents were ingested can ever pass; downstream retrieval over the crawled corpus is likewise unreachable.
Fix: the ingestion-job execution and statistics accounting in the bedrock-agent emulator — StartIngestionJob's handler and the job state it advances (read back by GetIngestionJob) must enumerate the bound data source's source documents (for an S3 data source, the objects under the configured bucketArn/prefix), index them into the knowledge base, and populate IngestionJobStatistics from that count instead of leaving the Smithy defaults in place; the same documents must then be visible through ListKnowledgeBaseDocuments for that data source.
Related already-open issue (does NOT cover this check): 20261029T195812Z-bedrock-agent-s3vectors-bedrock-agent-ingestion-into-an-s3-vectors-store.md reports zero numberOfNewDocumentsIndexed for an S3-Vectors-backed knowledge base under service token bedrock-agent__s3vectors; this failure is on the plain bedrock-agent path (OpenSearch-style minimal KB, numberOfDocumentsScanned) and is filed separately — a fixer should expect one accounting path to resolve both.
Traces: bedrock-agent.e2e-knowledge-base-ingestion-to-query-ready.

## Evidence
Check bedrock-agent.e2e-knowledge-base-ingestion-to-query-ready (verdict: mismatch).
AssertionError at check_bedrock_agent_e2e_knowledge_base_ingestion_to_query_ready.py line 103:
  assert final_job["statistics"]["numberOfDocumentsScanned"] >= 1
  "expected the crawl to scan at least the one S3 object written, got statistics={'numberOfDocumentsScanned': 0, 'numberOfMetadataDocumentsScanned': 0, 'numberOfNewDocumentsIndexed': 0, 'numberOfModifiedDocumentsIndexed': 0, 'numberOfMetadataDocumentsModified': 0, 'numberOfDocumentsDeleted': 0, 'numberOfDocumentsFailed': 0, 'numberOfDocumentsSkipped': 0}"
Expected: after PutObject of one text object into the data source's bucket and a StartIngestionJob that reaches status COMPLETE, statistics.numberOfDocumentsScanned >= 1 and that document accounted for across numberOfNewDocumentsIndexed / numberOfDocumentsFailed / numberOfDocumentsSkipped.
Actual: job status COMPLETE with every statistics member 0.
The job was accepted and reached a terminal state, and no error code or non-2xx was returned — the divergence is purely the emulator's accounting of the crawl (and, by implication, the absence of the crawl itself).

## Steps to Reproduce
0. git_clone, then run_behavioral_suite and read the bedrock-agent section (check e2e-knowledge-base-ingestion-to-query-ready).
1. CreateBucket an S3 bucket and create a minimal knowledge base bound to it (embedding model amazon.titan-embed-text-v1, dimension 1536).
2. CreateDataSource with dataSourceConfiguration {"type": "S3", "s3Configuration": {"bucketArn": "arn:<partition>:s3:::<bucket>"}} and wait for the data source to reach AVAILABLE.
3. PutObject one small text object (e.g. key sim2real-doc.txt) into that bucket.
4. StartIngestionJob(knowledgeBaseId, dataSourceId) and poll GetIngestionJob until the job reaches a terminal status; observe status COMPLETE.
5. Read the job's `statistics`: observe numberOfDocumentsScanned = 0 (and every other counter 0) where the one object written in step 3 must be reported as scanned and newly indexed.
6. ListKnowledgeBaseDocuments(knowledgeBaseId, dataSourceId) for the S3 data source: the crawled document is likewise absent.
