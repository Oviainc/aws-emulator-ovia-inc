Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent__s3vectors.data-deletion-policy-governs-vector-removal-at-the-store
Issue-Key: bedrock-agent__s3vectors.data-deletion-policy-governs-vector-removal-at-the-store|_run_ingestion_job#13|AssertionError|assert stats["numberOfNewDocumentsIndexed"] == document_count, (
Issue-Commit: 5b71589754f95f05f88c1a0ee9ccd691bbc5f701

# Bedrock Agent ingestion into an S3 Vectors store reports numberOfNewDocumentsIndexed=0 for a data source that holds documents, so no ingestion statistics can be trusted

> _Note: this issue was identified against the code base at commit `5b71589754f95f05f88c1a0ee9ccd691bbc5f701`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartIngestionJob

## Description
ONE root cause. Cause 1 of 1: an ingestion job run against a knowledge base whose storage is S3 Vectors completes, but its IngestionJobStatistics report that nothing was indexed, even though the bound S3 data source holds source documents.

AWS: Smithy (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/smithy-model/com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda.json) models IngestionJobStatistics on the ingestion job returned by StartIngestionJob / GetIngestionJob / ListIngestionJobs, with `numberOfDocumentsScanned` ("The total number of source documents that were scanned. Includes new, updated, and unchanged documents."), `numberOfNewDocumentsIndexed` ("The number of new source documents in the data source that were successfully indexed."), `numberOfModifiedDocumentsIndexed`, `numberOfDocumentsDeleted`, `numberOfDocumentsFailed` and `numberOfDocumentsSkipped` — every member a PrimitiveLong defaulting to 0. The counters are therefore the service's own accounting of what the job did, and a job that ingests N previously-unseen source documents from its data source and reaches a terminal COMPLETE state reports numberOfNewDocumentsIndexed == N (with the remainder accounted for across failed/skipped). Recorded behavior only for the exact split between the counters; the requirement exercised here is that a COMPLETE job over a data source holding N new documents does not report 0 new documents indexed while also reporting none failed and none skipped.
Emulator: the job is accepted and reaches its terminal state, but its statistics come back with numberOfNewDocumentsIndexed = 0 for a data source holding 2 documents. The check fails on its very first ingestion (the dataDeletionPolicy=RETAIN arm) before any policy behaviour is reached, so the emulator is not tracking ingested source documents into the statistics structure at all — the counters look like the Smithy defaults (all zero) rather than measured work.
Why: numberOfNewDocumentsIndexed is the signal every knowledge-base ingestion pipeline polls on to decide whether its sync actually did anything — it gates "retry the sync", "alert that the bucket prefix was empty", "proceed to query the KB" and all ingestion dashboards. Against the emulator a successful ingestion is indistinguishable from an ingestion that silently indexed nothing, so a client whose data source is genuinely empty or mis-prefixed is never caught, and any assertion that documents were indexed cannot pass. It also blocks every downstream behaviour that depends on vectors actually existing in the bound S3 Vectors index — including the dataDeletionPolicy RETAIN/DELETE semantics this check exists to pin, which are never reached.
Fix: the ingestion-job execution and its statistics accounting in the bedrock-agent emulator — the component that runs a data source's documents into the knowledge base's vector store (StartIngestionJob's handler and the job state it advances, read back by GetIngestionJob) must count the source documents it scanned and indexed and populate IngestionJobStatistics from that count instead of leaving the modeled defaults in place; the write into the bound S3 Vectors index is the other half, since the check's later legs read the index population back through s3vectors ListVectors.
Traces: bedrock-agent__s3vectors.data-deletion-policy-governs-vector-removal-at-the-store.

Note on attribution: this check's id carries the composite service token `bedrock-agent__s3vectors`; both halves (bedrock-agent and s3vectors) are bootstrapped, and the failing assertion is on a bedrock-agent ingestion-job response, so the defect is on the bedrock-agent side of the pair. The check is marked DRAFT/unblessed, so the dataDeletionPolicy hypotheses it carries are unconfirmed — but the failure filed here is upstream of all of them: it is the ingestion job's own reported statistics.

## Evidence
Check bedrock-agent__s3vectors.data-deletion-policy-governs-vector-removal-at-the-store, first ingestion (the dataDeletionPolicy=RETAIN arm): expected the job's IngestionJobStatistics.numberOfNewDocumentsIndexed to equal the 2 documents placed in the data source; actual numberOfNewDocumentsIndexed = 0.
AssertionError: RETAIN arm ingestion: expected all 2 documents to be indexed as new, got numberOfNewDocumentsIndexed=0
(raised at check_bedrock_agent__s3vectors_data_deletion_policy_governs_vector_removal_at_the_store.py line 507, from run() line 98 — i.e. on the first of the two ingestion arms, so the RETAIN/DELETE discrimination the check exists for is never reached.)
The job itself was accepted and advanced to its terminal state; no documents were reported failed or skipped either, so the 2 scanned documents are unaccounted for across every counter in the structure.

## Steps to Reproduce
0. git_clone, then run_behavioral_suite and read the bedrock-agent__s3vectors section.
1. Create an S3 Vectors vector bucket and an index to serve as the knowledge-base vector store, and an S3 bucket holding 2 small text documents under one prefix.
2. CreateKnowledgeBase with S3_VECTORS storage bound to that vector index.
3. CreateDataSource for the S3 prefix (dataDeletionPolicy=RETAIN), with FIXED_SIZE chunking.
4. StartIngestionJob for that data source and poll GetIngestionJob until the job reaches a terminal status.
5. Read the job's `statistics`: observe numberOfNewDocumentsIndexed = 0 (and numberOfDocumentsFailed / numberOfDocumentsSkipped also 0) where the 2 documents of the data source must be reported as newly indexed.
