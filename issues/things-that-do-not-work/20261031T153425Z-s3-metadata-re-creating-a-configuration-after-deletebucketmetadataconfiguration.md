Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.metadata-table-configuration-delete-removes-table
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.metadata-table-configuration-delete-removes-table|run#20|AssertionError|assert after_journal["TableStatus"] in ("CREATING", "ACTIVE"), (
Issue-Commit: 81ad204619d3c71dcf18bfdd43cea81955af6d23

# S3 Metadata: re-creating a configuration after DeleteBucketMetadataConfiguration lands the journal table in FAILED/JournalTableAlreadyExists instead of provisioning it afresh

> _Note: this issue was identified against the code base at commit `81ad204619d3c71dcf18bfdd43cea81955af6d23`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBucketMetadataConfiguration

## Description
Cause 1 of 1: after DeleteBucketMetadataConfiguration on a general purpose bucket whose journal table has not yet been provisioned, an immediately following CreateBucketMetadataConfiguration for the SAME bucket is recorded with its journal table already terminal-FAILED, so the bucket can never carry a working metadata configuration again. Failing check: s3.metadata-table-configuration-delete-removes-table.

AWS: recorded behavior only for this flow — the S3 model declares no error shape for CreateBucketMetadataConfiguration, DeleteBucketMetadataConfiguration or GetBucketMetadataConfiguration (Smithy: all three have errors: []; Create is POST /{Bucket}?metadataConfiguration with @http code 200 and a required MetadataConfiguration payload, Delete is DELETE /{Bucket}?metadataConfiguration code 204, Get is GET /{Bucket}?metadataConfiguration code 200 with the GetBucketMetadataConfigurationResult payload), so a provisioning outcome is reported only through GetBucketMetadataConfigurationResult.MetadataConfigurationResult.JournalTableConfigurationResult.TableStatus plus its ErrorDetails. The recording for this check shows: the first CreateBucketMetadataConfiguration with MetadataConfiguration={JournalTableConfiguration:{RecordExpiration:{Expiration:"DISABLED"}}} is accepted and the journal table is reported in a non-terminal/healthy status (TableStatus is CREATING or ACTIVE) with RecordExpiration.Expiration = "DISABLED"; DeleteBucketMetadataConfiguration then makes the configuration gone, so GetBucketMetadataConfiguration and UpdateBucketMetadataJournalTableConfiguration both refuse with Code "MetadataConfigurationNotFound" and Message "The metadata configuration was not found" (that Message is entirely FIXED text for this error class — it carries no request- or state-derived values; only the bucket in the request URI varies, and it does not appear in the message); and the bucket is then REUSABLE — an identical CreateBucketMetadataConfiguration is accepted and its journal table is again reported with TableStatus in (CREATING, ACTIVE) and RecordExpiration.Expiration = "DISABLED", i.e. no ErrorDetails and no JournalTableAlreadyExists. Hypothesis: AWS commits a metadata table's name into the AWS-managed table bucket namespace only once that table has actually been provisioned, so a configuration deleted while its journal table is still being created leaves no table behind and a prompt re-create provisions afresh; the leftover-table conflict (ErrorDetails$ErrorCode "JournalTableAlreadyExists", documented in the V2 list) is reachable only when the earlier journal table really reached ACTIVE, which is the distinct flow the still-open issue 20261028T234552Z-s3-deleting-an-s3-metadata-configuration-also-frees.md describes (it polls to ACTIVE before deleting). That issue therefore does NOT cover this check: it argues the conflict must be reachable at all, while this check requires the delete-then-recreate path NOT to be universally poisoned. The open issues 20261027T001031Z-s3-deletebucketmetadataconfiguration-stub.md, 20261028T141517Z-s3-updatebucketmetadataannotationtableconfiguration-is-an-unimplemented-scaffold-stub-501.md and 20261030T053639Z-s3-getbucketmetadataconfiguration-misreports-table-names-namespace-and-configuration.md are all about other operations or other members and do not cover it either.

Emulator: the re-created journal table is reported terminal-FAILED. On replay the check's first three stages all passed — the first create reached CREATING/ACTIVE, and both the post-delete GetBucketMetadataConfiguration and the post-delete UpdateBucketMetadataJournalTableConfiguration refused with MetadataConfigurationNotFound / "The metadata configuration was not found" — and it failed only on the re-create, where GetBucketMetadataConfigurationResult.MetadataConfigurationResult.JournalTableConfigurationResult.TableStatus came back as "FAILED" (AssertionError: recreated journal TableStatus 'FAILED'). The reason is that CreateBucketMetadataConfiguration records the derived table names into the managed-namespace set synchronously at create time — before any table is ACTIVE — and DeleteBucketMetadataConfiguration deliberately never releases them, so metadata_namespace_holds(journal_name) is true for every subsequent create on that bucket and the handler builds the journal table with MetadataTable::failed(..., METADATA_JOURNAL_TABLE_EXISTS_CODE, ...). The effect is permanent for the lifetime of the store: no S3 API in this crate releases the name, so once a bucket has had any metadata configuration created, every later configuration on that bucket is born FAILED.

Why: a client relying on AWS's behavior loses the documented recovery path. AWS's own JournalTableAlreadyExists guidance is "delete the journal table, and then try again" — the conflict is supposed to be a condition the caller can clear, and a configuration deleted before its tables were ever provisioned is supposed to leave nothing to clear. Against the emulator, delete-then-recreate is a one-way door: a caller that tears a metadata configuration down and stands it back up (a redeploy, a test fixture, a configuration change expressed as delete+create) gets a configuration that is recorded but whose journal table is terminal-FAILED with JournalTableAlreadyExists, so no metadata journal is ever produced and the caller's retry/cleanup logic cannot recover. Clients also disagree about reality: GetBucketMetadataConfiguration reports a dead journal table while the s3tables managed namespace holds a name the caller cannot see or delete through S3.

Fix: the managed-namespace bookkeeping in the s3 service state behind these two handlers — namely when CreateBucketMetadataConfiguration commits a table name into the namespace and when a configuration delete may release it. A name must be held by a table that actually exists, not by a create that merely recorded one, so a configuration whose tables were never provisioned must not leave its names taken; and the name-taken branch that mints MetadataTable::failed(JournalTableAlreadyExists) must fire only for a name a provisioned table really holds. This belongs in ComAmazonawsS3Amazons3State (record_metadata_namespace_table / metadata_namespace_holds / delete_metadata_configuration) and in the create handler's provisioning path, not in the serializer.

Traces: s3.metadata-table-configuration-delete-removes-table. Emulator source: services/service-com-amazonaws-s3-amazons3/src/ops/create_bucket_metadata_configuration.rs:372-384 (journal_taken -> MetadataTable::failed(..., METADATA_JOURNAL_TABLE_EXISTS_CODE ...)) and :459-507 (the names a create records into the namespace, committed before any table is ACTIVE); services/service-com-amazonaws-s3-amazons3/src/ops/delete_bucket_metadata_configuration.rs:193-220 (the delete removes only the configuration and documents that the tables' names stay held for the life of the store).

## Evidence
Check s3.metadata-table-configuration-delete-removes-table [mismatch].

Failure detail from the replay:

  File "check_s3_metadata_table_configuration_delete_removes_table.py", line 52, in run
    assert after_journal["TableStatus"] in ("CREATING", "ACTIVE"), (
  AssertionError: recreated journal TableStatus 'FAILED'

Assertions that PASSED before it, so they are not in question:
- first CreateBucketMetadataConfiguration(Bucket=B, MetadataConfiguration={"JournalTableConfiguration": {"RecordExpiration": {"Expiration": "DISABLED"}}}) accepted; GetBucketMetadataConfiguration -> JournalTableConfigurationResult.TableStatus in (CREATING, ACTIVE) and RecordExpiration.Expiration == "DISABLED".
- DeleteBucketMetadataConfiguration(Bucket=B) accepted.
- GetBucketMetadataConfiguration(Bucket=B) -> Code "MetadataConfigurationNotFound", Message "The metadata configuration was not found".
- UpdateBucketMetadataJournalTableConfiguration(Bucket=B, JournalTableConfiguration={"RecordExpiration": {"Expiration": "ENABLED", "Days": 30}}) -> the same Code and Message.

Expected (from the recording), read from GetBucketMetadataConfigurationResult.MetadataConfigurationResult.JournalTableConfigurationResult after the second, identical create:
  TableStatus: "CREATING" or "ACTIVE"
  RecordExpiration.Expiration: "DISABLED"
  (no ErrorDetails)

Actual (emulator):
  TableStatus: "FAILED"
  with the handler's name-taken failure, ErrorDetails ErrorCode "JournalTableAlreadyExists"

Smithy grounding (services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json): CreateBucketMetadataConfiguration — POST /{Bucket}?metadataConfiguration, code 200, required httpPayload MetadataConfiguration, errors: []; DeleteBucketMetadataConfiguration — DELETE /{Bucket}?metadataConfiguration, code 204, errors: []; GetBucketMetadataConfiguration — GET /{Bucket}?metadataConfiguration, code 200, httpPayload GetBucketMetadataConfigurationResult, errors: []. Because none of the three models an error, the provisioning verdict exists only as JournalTableConfigurationResult.TableStatus + ErrorDetails, which is exactly the field that diverges.

Emulator source grounding:
- services/service-com-amazonaws-s3-amazons3/src/ops/create_bucket_metadata_configuration.rs:372-384 — let journal_taken = store.metadata_namespace_holds(&journal_name); true => MetadataTable::failed(journal_name, now, { code: METADATA_JOURNAL_TABLE_EXISTS_CODE, message: METADATA_JOURNAL_TABLE_EXISTS_MESSAGE }).
- services/service-com-amazonaws-s3-amazons3/src/ops/create_bucket_metadata_configuration.rs:459-507 — the create records every table name it provisions into the namespace immediately ("a table OUTLIVES the configuration that created it ... whether that name was taken by another bucket's configuration or by this bucket's own earlier one"), i.e. the name is committed while TableStatus is still CREATING.
- services/service-com-amazonaws-s3-amazons3/src/ops/delete_bucket_metadata_configuration.rs:193-220 — "The TABLES the configuration provisioned stay where they are ... A create of the same configuration afterwards is therefore accepted and its journal table reports that failure": nothing in this crate ever releases the name.

Related but not covering: open issue 20261028T234552Z-s3-deleting-an-s3-metadata-configuration-also-frees.md is grounded in the sibling check s3.create-bucket-metadata-table-configuration-name-conflict-rejected, which polls the first journal table to ACTIVE before deleting the configuration and which PASSES on this replay. The two checks are distinguished by whether the first journal table was ever provisioned; the emulator applies the name-taken verdict unconditionally and so cannot satisfy both.

## Steps to Reproduce
1. CreateBucket on a general purpose bucket B (pass CreateBucketConfiguration.LocationConstraint=<region> when the region is not us-east-1).
2. CreateBucketMetadataConfiguration(Bucket=B, MetadataConfiguration={"JournalTableConfiguration": {"RecordExpiration": {"Expiration": "DISABLED"}}}).
3. GetBucketMetadataConfiguration(Bucket=B) once, WITHOUT polling to ACTIVE, and confirm MetadataConfigurationResult.JournalTableConfigurationResult.TableStatus is CREATING or ACTIVE and RecordExpiration.Expiration is "DISABLED".
4. DeleteBucketMetadataConfiguration(Bucket=B).
5. Confirm GetBucketMetadataConfiguration(Bucket=B) and UpdateBucketMetadataJournalTableConfiguration(Bucket=B, JournalTableConfiguration={"RecordExpiration": {"Expiration": "ENABLED", "Days": 30}}) both refuse with Code "MetadataConfigurationNotFound" and Message "The metadata configuration was not found". (These pass today.)
6. CreateBucketMetadataConfiguration(Bucket=B, ...) again with the identical MetadataConfiguration. Do not touch the AWS-managed table bucket namespace (arn:aws:s3tables:<region>:<account>:bucket/aws-s3, namespace b_<B>) in between.
7. GetBucketMetadataConfiguration(Bucket=B) and read MetadataConfigurationResult.JournalTableConfigurationResult.
   Expected: TableStatus "CREATING" or "ACTIVE", RecordExpiration.Expiration "DISABLED", no ErrorDetails.
   Observed: TableStatus "FAILED" with ErrorDetails ErrorCode "JournalTableAlreadyExists", and every further create on B is born FAILED the same way.
