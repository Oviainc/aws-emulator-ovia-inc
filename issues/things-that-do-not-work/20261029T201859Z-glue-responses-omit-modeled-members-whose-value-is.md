Issue-Source: trace-sweep-divergence
Issue-Trace: glue-classifier-custom-entity-and-data-quality-ruleset
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-classifier-custom-entity-and-data-quality-ruleset glue-classifier-lifecycle glue-classifier-and-security-configuration-listing glue-security-configuration-lifecycle glue-database-table-partition-lifecycle glue-partition-index-and-partition-column-statistics glue-schema-version-lifecycle glue-schema-versions-lifecycle glue-trigger-scheduled-with-workflow
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetClassifier|2xx->2xx|$.Classifier.CsvClassifier.Serde
Issue-Key: glue|GetClassifiers|2xx->2xx|$.Classifiers[*]
Issue-Key: glue|GetSecurityConfigurations|2xx->2xx|$.SecurityConfigurations[Name="sim2real-probe-glue-glue-secconf-c65a675d"].EncryptionConfiguration.EnforceJdbcSsl
Issue-Key: glue|GetSecurityConfiguration|2xx->2xx|$.SecurityConfiguration.EncryptionConfiguration.EnforceJdbcSsl
Issue-Key: glue|GetDataCatalogEncryptionSettings|2xx->2xx|$.DataCatalogEncryptionSettings.EncryptedCatalogSearchStatus
Issue-Key: glue|BatchGetPartition|2xx->2xx|$.UnprocessedKeys
Issue-Key: glue|DeleteSchemaVersions|2xx->2xx|$
Issue-Key: glue|GetWorkflow|2xx->2xx|$.Workflow.Graph.Nodes[*].JobDetails
Issue-Related: 20261029T201859Z-glue-delete-and-update-mutations-are-never-applied.md
Issue-Commit: c912e232849d0cf345219eaaac304bd62b7f7616

# Glue responses omit modeled members whose value is a default or empty collection

> _Note: this issue was identified against the code base at commit `c912e232849d0cf345219eaaac304bd62b7f7616`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetClassifier

## Description
This issue bundles 9 net-new causes with one shared fault: the emulator's serializers omit any member whose value is a default, a sentinel or an empty collection, where AWS emits the member. Status and error codes are right, so this is MEDIUM — but a client cannot distinguish 'empty' from 'absent', and SDK code that dereferences the member fails.

Cause 1 of 9: GetClassifier omits CsvClassifier.Serde.
AWS: Smithy: CsvClassifier.Serde targets the CsvSerdeOption enum whose documented values include 'None' ('You can specify the None value when you want the crawler to do the detection'). AWS returns Serde with the enum value the classifier carries — 'None' is a value, not an absence.
Emulator: the member is absent.
Why: a client reading the SerDe choice cannot tell it was explicitly set to None.
Fix: GetClassifier serializer (emit Serde, including the None enum value).
Traces: glue-classifier-custom-entity-and-data-quality-ruleset#task#5 (also #task#7, glue-classifier-lifecycle#task#5).

Cause 2 of 9: GetClassifiers entries omit Serde (and render timestamps differently).
AWS: each Classifiers[] entry is the same CsvClassifier structure as cause 1, including Serde.
Emulator: the listed entry has no Serde member.
Why: as cause 1, over listings.
Fix: the shared classifier serializer used by GetClassifiers.
Traces: glue-classifier-lifecycle#task#3.

Cause 3 of 9: GetSecurityConfigurations omits EncryptionConfiguration.EnforceJdbcSsl.
AWS: Smithy models EncryptionConfiguration with S3Encryption/CloudWatchEncryption/JobBookmarksEncryption/DataQualityEncryption only, so EnforceJdbcSsl is recorded behavior only: AWS returns it inside each listed configuration's EncryptionConfiguration, false when JDBC SSL was not enforced.
Emulator: the member is absent.
Why: a client auditing JDBC SSL enforcement reads nothing rather than an explicit false.
Fix: the security-configuration serializer used by GetSecurityConfigurations.
Traces: glue-classifier-and-security-configuration-listing#task#4.

Cause 4 of 9: GetSecurityConfiguration omits the same member on the single read.
AWS: as cause 3, under SecurityConfiguration.EncryptionConfiguration.
Emulator: absent.
Why: as cause 3.
Fix: the same serializer on the single-read path.
Traces: glue-security-configuration-lifecycle#task#3.

Cause 5 of 9: GetDataCatalogEncryptionSettings omits EncryptedCatalogSearchStatus.
AWS: recorded behavior only — the settings carry DataCatalogEncryptionSettings.EncryptedCatalogSearchStatus, whose value is the fixed enum 'NOT_ENABLED' when encrypted catalog search has not been turned on.
Emulator: the member is absent.
Why: a client checking catalog-search encryption state cannot.
Fix: GetDataCatalogEncryptionSettings serializer.
Traces: glue-security-configuration-lifecycle#task#4.

Cause 6 of 9: GetPartition omits StorageDescriptor.Columns when the list is empty.
AWS: the partition's StorageDescriptor always carries Columns; an unset column list is returned as the empty list.
Emulator: the member is absent.
Why: SDK code that iterates Columns without a null check breaks, and a client cannot distinguish 'no columns' from 'not reported'.
Fix: the StorageDescriptor serializer used by GetPartition.
Traces: glue-database-table-partition-lifecycle#task#13.

Cause 7 of 9: BatchGetPartition omits UnprocessedKeys when nothing was unprocessed.
AWS: Smithy: BatchGetPartition output models UnprocessedKeys (BatchGetPartitionValueList, length 0..1000) — AWS returns the empty list when every requested key resolved.
Emulator: the member is absent.
Why: a batch caller's standard retry loop reads UnprocessedKeys to decide whether to re-request; an absent member forces it to special-case.
Fix: BatchGetPartition serializer.
Traces: glue-partition-index-and-partition-column-statistics#task#13 (also #task#15).

Cause 8 of 9: DeleteSchemaVersions omits SchemaVersionErrors.
AWS: Smithy: DeleteSchemaVersions output models SchemaVersionErrors (SchemaVersionErrorList) — AWS returns the empty list when all requested versions deleted cleanly.
Emulator: answers 200 with a completely empty body.
Why: a client that inspects per-version errors cannot distinguish success from an unparsed response.
Fix: DeleteSchemaVersions serializer.
Traces: glue-schema-version-lifecycle#task#5 (also glue-schema-versions-lifecycle#task#3).

Cause 9 of 9: GetWorkflow omits Graph.Nodes[].JobDetails for a job node with no runs.
AWS: each workflow graph node carries its kind-specific details; a job node with no runs yet has JobDetails present as an empty structure.
Emulator: the member is absent on the job node.
Why: a client walking the workflow graph and reading JobDetails.JobRuns per node breaks on the node it cares about.
Fix: the workflow-graph node serializer used by GetWorkflow.
Traces: glue-trigger-scheduled-with-workflow#task#6 (also #task#7).

## Evidence
Replay of simparity-wire-aws-v1, all steps 200/200:
1. GetClassifier: $.Classifier.CsvClassifier.Serde missing (expected 'None', got None).
2. GetClassifiers: $.Classifiers[0] differs — expected entry includes 'Serde': 'None'; the returned entry has no Serde member.
3. GetSecurityConfigurations: $.SecurityConfigurations[Name=...].EncryptionConfiguration.EnforceJdbcSsl missing (expected False).
4. GetSecurityConfiguration: $.SecurityConfiguration.EncryptionConfiguration.EnforceJdbcSsl missing (expected False).
5. GetDataCatalogEncryptionSettings: $.DataCatalogEncryptionSettings.EncryptedCatalogSearchStatus missing (expected 'NOT_ENABLED').
6. GetPartition: $.Partition.StorageDescriptor.Columns missing (expected []).
7. BatchGetPartition: $.UnprocessedKeys missing (expected []).
8. DeleteSchemaVersions: $ empty_body (expected {'SchemaVersionErrors': []}, got {}).
9. GetWorkflow: $.Workflow.Graph.Nodes[1].JobDetails missing (expected {}).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Re-replay: glue-classifier-custom-entity-and-data-quality-ruleset#task#5, glue-classifier-lifecycle#task#3, glue-classifier-and-security-configuration-listing#task#4, glue-security-configuration-lifecycle#task#3, glue-security-configuration-lifecycle#task#4, glue-database-table-partition-lifecycle#task#13, glue-partition-index-and-partition-column-statistics#task#13, glue-schema-version-lifecycle#task#5, glue-trigger-scheduled-with-workflow#task#6.
3. Each step reads the resource after creating it and asserts the member named in its cause is present with its default/empty value.
