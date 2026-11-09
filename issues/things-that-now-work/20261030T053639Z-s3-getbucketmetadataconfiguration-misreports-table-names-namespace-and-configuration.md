Issue-Source: trace-sweep-divergence
Issue-Trace: s3-bucket-metadata-and-metadata-table-configuration-management
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-bucket-metadata-and-metadata-table-configuration-management s3-bucket-metadata-configuration-crud-lifecycle s3-metadata-configuration-lifecycle s3-metadata-update-on-missing-config-errors s3-metadata-v2-config-lifecycle s3-metadata-config-with-inventory-and-annotation s3-metadata-config-with-sse-encryption s3-metadata-configuration-duplicate-create-error s3-metadata-configuration-invalid-record-expiration-error s3-metadata-configuration-journal-and-inventory s3-metadata-configuration-missing-config-errors
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|GetBucketMetadataConfiguration|2xx->2xx|$.GetBucketMetadataConfigurationResult.MetadataConfigurationResult.AnnotationTableConfigurationResult
Issue-Key: s3|GetBucketMetadataConfiguration|2xx->2xx|$.GetBucketMetadataConfigurationResult.MetadataConfigurationResult.DestinationResult.TableNamespace
Issue-Key: s3|GetBucketMetadataConfiguration|2xx->2xx|$.GetBucketMetadataConfigurationResult.MetadataConfigurationResult.InventoryTableConfigurationResult
Issue-Key: s3|GetBucketMetadataConfiguration|2xx->2xx|$.GetBucketMetadataConfigurationResult.MetadataConfigurationResult.JournalTableConfigurationResult.TableName
Issue-Key: s3|GetBucketMetadataConfiguration|2xx->2xx|$.GetBucketMetadataConfigurationResult.MetadataConfigurationResult.InventoryTableConfigurationResult.ConfigurationState
Issue-Key: s3|GetBucketMetadataConfiguration|2xx->2xx|$.GetBucketMetadataConfigurationResult.MetadataConfigurationResult.JournalTableConfigurationResult.RecordExpiration.Days
Issue-Key: s3|GetBucketMetadataConfiguration|2xx->2xx|$.GetBucketMetadataConfigurationResult.MetadataConfigurationResult.AnnotationTableConfigurationResult.Error
Issue-Key: s3|GetBucketMetadataConfiguration|2xx->2xx|$.GetBucketMetadataConfigurationResult.MetadataConfigurationResult.AnnotationTableConfigurationResult.TableName
Issue-Key: s3|GetBucketMetadataConfiguration|2xx->2xx|$.GetBucketMetadataConfigurationResult.MetadataConfigurationResult.AnnotationTableConfigurationResult.TableStatus
Issue-Key: s3|GetBucketMetadataConfiguration|2xx->2xx|$.GetBucketMetadataConfigurationResult.MetadataConfigurationResult.InventoryTableConfigurationResult.TableName
Issue-Key: s3|DeleteBucketMetadataConfiguration|4xx->2xx|$
Issue-Key: s3|DeleteBucketMetadataConfiguration|4xx->2xx|$headers.content-length
Issue-Key: s3|DeleteBucketMetadataConfiguration|4xx->2xx|status
Issue-Commit: 90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1

# S3 GetBucketMetadataConfiguration misreports table names, namespace and configuration state

> _Note: this issue was identified against the code base at commit `90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketMetadataConfiguration

## Description
Six net-new root causes in the S3 Metadata configuration surface: GetBucketMetadataConfiguration invents its own table names and namespace, omits the disabled-table result containers, never reflects an applied update, reports a FAILED annotation table as ACTIVE, and DeleteBucketMetadataConfiguration answers 204 where AWS answers 404. A fixer must close all six.

AWS (common): GetBucketMetadataConfigurationResult.MetadataConfigurationResult reports the S3-managed table layout: JournalTableConfigurationResult.TableName is the fixed literal 'journal', InventoryTableConfigurationResult.TableName is 'inventory', AnnotationTableConfigurationResult.TableName is 'annotation', and DestinationResult.TableNamespace is the bucket's own namespace, i.e. 'b_' followed by the bucket name. Every optional table container is present whatever its state: a table that was not requested is reported as {ConfigurationState: DISABLED}. Smithy models MetadataConfigurationResult and its Journal/Inventory/Annotation result members; the literal table names and the 'b_<bucket>' namespace are recorded behavior.

Why (common): these are the coordinates a client uses to query the metadata tables — it builds the Iceberg/table reference from TableNamespace plus TableName. Emulator-minted names such as '<bucket_with_underscores>_journal' in a shared namespace 'aws_s3_metadata' point at nothing, and a missing DISABLED container makes a client that iterates the three result members crash or conclude a table exists when it does not.

Cause 1 of 6: journal-only configurations report the wrong TableName and omit the Inventory and Annotation DISABLED containers.
Emulator: JournalTableConfigurationResult.TableName is '<bucket-with-underscores>_journal'; InventoryTableConfigurationResult and AnnotationTableConfigurationResult are absent entirely; DestinationResult.TableNamespace is the fixed string 'aws_s3_metadata'.
Fix: the GetBucketMetadataConfiguration serializer and the metadata-configuration state it reads (table naming, namespace derivation, and always emitting the three result containers).
Traces: s3-bucket-metadata-and-metadata-table-configuration-management#task#3 (also s3-bucket-metadata-configuration-crud-lifecycle#task#3, s3-metadata-configuration-lifecycle#task#4, s3-metadata-update-on-missing-config-errors#task#6, s3-metadata-v2-config-lifecycle#task#3).

Cause 2 of 6: an applied journal/inventory update is not reflected in the next read.
AWS: after the update, InventoryTableConfigurationResult.ConfigurationState reads back DISABLED and JournalTableConfigurationResult.RecordExpiration.Days reads back the value just written.
Emulator: reports ConfigurationState 'ENABLED' and the pre-update Days, i.e. the update never reached the stored configuration.
Fix: the UpdateBucketMetadataJournalTableConfiguration / UpdateBucketMetadataInventoryTableConfiguration handlers' writes into metadata-configuration state.
Traces: s3-bucket-metadata-and-metadata-table-configuration-management#task#7.

Cause 3 of 6: an annotation table that failed to create is reported ACTIVE with no Error.
AWS: AnnotationTableConfigurationResult.TableStatus is FAILED and the result carries an Error with ErrorCode 'InsufficientAnnotationRolePermissions' and AWS's fixed explanatory message about the annotation role's required permissions.
Emulator: TableStatus 'ACTIVE' and no Error member; the table names are also the minted ones from cause 1.
Fix: the annotation-table creation path and the GetBucketMetadataConfiguration serializer's TableStatus/Error reporting.
Traces: s3-metadata-config-with-inventory-and-annotation#task#4.

Cause 4 of 6: an SSE-encrypted journal-only configuration shows the same naming and missing-container defects.
Emulator: minted journal TableName; Inventory and Annotation DISABLED containers absent.
Fix: as cause 1 — the read path is shared, but this configuration variant must be covered.
Traces: s3-metadata-config-with-sse-encryption#task#3 (also s3-metadata-configuration-duplicate-create-error#task#4, s3-metadata-configuration-invalid-record-expiration-error#task#4).

Cause 5 of 6: a journal+inventory configuration reports both minted table names, the wrong namespace and no Annotation container.
Emulator: InventoryTableConfigurationResult.TableName '<bucket>_inventory' instead of 'inventory', journal likewise, TableNamespace 'aws_s3_metadata' instead of 'b_<bucket>', no AnnotationTableConfigurationResult.
Fix: as cause 1, for the journal+inventory shape.
Traces: s3-metadata-configuration-journal-and-inventory#task#3.

Cause 6 of 6: DeleteBucketMetadataConfiguration on a bucket with no configuration answers 204 instead of 404.
AWS: 404 with Code 'MetadataConfigurationNotFound', message 'The metadata configuration was not found' (fixed text) and <BucketName> naming the bucket; the response also carries content-length.
Emulator: 204 with an empty body and no content-length.
Fix: the DeleteBucketMetadataConfiguration handler's not-found path.
Traces: s3-bucket-metadata-and-metadata-table-configuration-management#teardown#9 (also s3-metadata-configuration-lifecycle#task#7, s3-metadata-configuration-missing-config-errors#task#3)

## Evidence
All six clusters are 'mismatch'.
s3-bucket-metadata-and-metadata-table-configuration-management#task#3: InventoryTableConfigurationResult missing (expected {'ConfigurationState': 'DISABLED'}); AnnotationTableConfigurationResult missing (expected {'ConfigurationState': 'DISABLED'}); JournalTableConfigurationResult.TableName value_mismatch (expected 'journal', got 'sim2real_probe_s3_s3_metadata_92203541_journal'); DestinationResult.TableNamespace value_mismatch (expected 'b_<bucket>', got 'aws_s3_metadata')
s3-bucket-metadata-and-metadata-table-configuration-management#task#7: InventoryTableConfigurationResult.ConfigurationState value_mismatch (expected 'DISABLED', got 'ENABLED'); JournalTableConfigurationResult.RecordExpiration.Days value_mismatch (expected '7', got '14'); plus the naming/namespace/container defects
s3-metadata-config-with-inventory-and-annotation#task#4: AnnotationTableConfigurationResult.TableStatus value_mismatch (expected 'FAILED', got 'ACTIVE'); AnnotationTableConfigurationResult.Error missing (expected ErrorCode 'InsufficientAnnotationRolePermissions' with AWS's annotation-role message); Inventory/Journal/Annotation TableName and TableNamespace all minted
s3-metadata-config-with-sse-encryption#task#3: Inventory/Annotation containers missing; journal TableName minted
s3-metadata-configuration-journal-and-inventory#task#3: Inventory and Journal TableName minted; TableNamespace 'aws_s3_metadata'; Annotation container missing
s3-bucket-metadata-and-metadata-table-configuration-management#teardown#9 (DeleteBucketMetadataConfiguration): status expected 404, got 204; body empty_body (expected MetadataConfigurationNotFound 'The metadata configuration was not found' with BucketName); $headers.content-length missing

## Steps to Reproduce
Replay the wire corpus and re-run the fixture#step bindings in Issue-Trace/Issue-Step and trace_ids. By hand: CreateBucketMetadataConfiguration with a journal table only, then GET /<bucket>?metadataConfiguration and check TableName == 'journal', TableNamespace == 'b_<bucket>' and that Inventory/Annotation results are present as {ConfigurationState: DISABLED}; repeat with journal+inventory and with an annotation table whose role lacks permissions; apply an inventory-disable plus a journal RecordExpiration change and re-read; finally DELETE /<bucket>?metadataConfiguration on a bucket that has no configuration and check for the 404 MetadataConfigurationNotFound envelope.
