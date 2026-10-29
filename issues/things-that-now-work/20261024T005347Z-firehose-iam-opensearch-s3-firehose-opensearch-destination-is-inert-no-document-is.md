Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-fgac-strict-mapping-routes-conflicts-to-s3 firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-unmapped-role-denied-all-records-to-s3
Issue-Key: firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-fgac-strict-mapping-routes-conflicts-to-s3|_wait_master#15|Timeout|time.sleep(10.0)
Issue-Key: firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-unmapped-role-denied-all-records-to-s3|_wait_master#15|Timeout|time.sleep(10.0)
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# Firehose OpenSearch destination is inert: no document is indexed and no rejected document reaches the S3 backup

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecordBatch

## Description
One net-new root cause behind the firehose__iam__opensearch__s3 failures.

Cause 1 of 1: records accepted for a stream with an OpenSearch destination are never indexed and the documents that OpenSearch refuses are never written to the stream's S3 backup, because the Firehose destination-delivery path does not exist.
AWS: Smithy Firehose PutRecord doc: "Firehose buffers records before delivering them to the destination"; AmazonopensearchserviceDestinationConfiguration carries DomainARN/IndexName/RoleARN together with S3BackupMode and an S3Configuration, and a document OpenSearch rejects (strict field mapping conflict, or an FGAC role with no mapping, i.e. a 403) is delivered to that S3 backup under the error prefix instead of being dropped. So on real AWS the mapped records become searchable documents while the conflicting/denied ones appear as objects in the backup bucket.
Emulator: PutRecordBatch answers 200 with a RecordId per record, but the data is only appended to an in-process buffer (services/service-com-amazonaws-firehose-firehose-20150804/src/state.rs, DeliveryStream.records, drained only by the 24 h RECORD_RETENTION expiry); the crate declares no delivery component (src/lib.rs: ops, scaffolded, service, state), so it never calls OpenSearch and never writes to S3. Both checks poll their observables until the suite budget expires ("Error: Check timed out after 60.0s").
Why: the whole log-ingestion-into-OpenSearch pipeline is a silent drop. A client cannot see indexed documents, cannot distinguish an IAM/FGAC misconfiguration (which on AWS surfaces as rejected documents parked in S3) from a healthy stream, and loses the recovery channel the S3 backup exists to provide.
Fix: the Firehose destination-delivery (flush) component, with an OpenSearch destination writer that indexes buffered records into the OpenSearch service's state and routes per-document failures (mapping conflicts, authorization denials) to the destination's S3 backup prefix.
Traces: firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-fgac-strict-mapping-routes-conflicts-to-s3, firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-unmapped-role-denied-all-records-to-s3.

## Evidence
Both checks: "Error: Check timed out after 60.0s".
Expected (real AWS): the conforming records become documents retrievable from the OpenSearch index, while the mapping-conflicting documents (and, for an unmapped FGAC role, every document) appear as objects under the stream's S3 backup error prefix.
Actual (emulator): PutRecordBatch returns 200 with RecordIds; the OpenSearch index receives nothing and the S3 backup bucket stays empty, so neither the positive barrier nor the error-routing observable ever appears.

## Steps to Reproduce
1. Boot the emulator; create an OpenSearch domain, an S3 backup bucket and an IAM role trusted by firehose.amazonaws.com granting es: and s3: access.
2. firehose.create_delivery_stream with an Amazon OpenSearch destination (DomainARN, IndexName, RoleARN, S3BackupMode='FailedDocumentsOnly', S3Configuration -> the backup bucket with an error prefix); wait for ACTIVE.
3. put_record_batch with documents that conform to the index mapping plus documents that conflict with it.
4. Expect the conforming documents to be searchable in the index and the conflicting ones to appear under the S3 backup error prefix; observe that neither happens — the index stays empty and list_objects_v2 on the backup bucket returns nothing.
5. Re-replay the check ids in trace_ids.
