Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.create-bucket-metadata-table-configuration-name-conflict-rejected
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.create-bucket-metadata-table-configuration-name-conflict-rejected|run#19|AssertionError|assert restored_status == "FAILED", (
Issue-Commit: 7f38edfb17ee58eefb2619194500340afad2c5d5

# s3: deleting an S3 Metadata configuration also frees its journal table name in the AWS-managed table bucket namespace, so an identical re-create reaches ACTIVE instead of FAILED with JournalTableAlreadyExists

> _Note: this issue was identified against the code base at commit `7f38edfb17ee58eefb2619194500340afad2c5d5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBucketMetadataConfiguration

## Description
Cause 1 of 1: DeleteBucketMetadataConfiguration releases the metadata table names its configuration provisioned in the AWS-managed table bucket namespace, so the journal-table-name-taken provisioning failure is unreachable for the bucket that just had its configuration deleted.

AWS: S3 Metadata V2 draws a line between the metadata CONFIGURATION on a general purpose bucket and the TABLES that configuration provisions in the AWS-managed table bucket's namespace. DeleteBucketMetadataConfiguration removes only the configuration; the journal table it created stays in the managed namespace and must be deleted separately through s3tables. Consequently a subsequent CreateBucketMetadataConfiguration for the SAME bucket is accepted synchronously (HTTP 200, empty body — the operation models no error for this and the conflict is a provisioning outcome, not a request refusal), and asynchronous provisioning then fails: GetBucketMetadataConfiguration reports GetBucketMetadataConfigurationResult.MetadataConfigurationResult.JournalTableConfigurationResult.TableStatus = "FAILED" carrying ErrorDetails with ErrorCode "JournalTableAlreadyExists". Smithy: the s3 model documents that code on ErrorDetails$ErrorCode in the V2 CreateBucketMetadataConfiguration list — "JournalTableAlreadyExists - A journal table already exists in the Amazon Web Services managed table bucket's namespace. Delete the journal table, and then try again. To create a new metadata table, you must delete the metadata configuration for this bucket, and then create a new metadata configuration." The recorded live ErrorMessage for this class is the same rule in the service's own wording: "A journal table already exists in the AWS managed table bucket's namespace. Delete the journal table, and then try again. To create a new S3 Metadata table, you must delete the S3 Metadata configuration for this bucket, and then create a new S3 Metadata configuration." All of that text is FIXED text for this error class — it carries no request- or state-derived values at all; the only per-request parts of the response are the bucket named in the request URI and the table name the service derives from it, neither of which appears in the message. Note this is distinct from the configuration-scoped request refusal the same check exercises earlier and which the emulator already answers correctly: creating a second configuration while the first is still ACTIVE is refused with MetadataConfigurationAlreadyExists / HTTP 409 / "The metadata configuration already exists".

Emulator: the configuration delete and the namespace release are performed together, so the name is free immediately. services/service-com-amazonaws-s3-amazons3/src/ops/delete_bucket_metadata_configuration.rs calls store.delete_metadata_configuration(&input.bucket), documented in that handler as "The TABLES the configuration created go with it ... so their names are free again and a fresh CreateBucketMetadataConfiguration on the same bucket provisions its tables rather than finding their names taken." The create handler's name-taken branch (services/service-com-amazonaws-s3-amazons3/src/ops/create_bucket_metadata_configuration.rs, store.metadata_namespace_holds(&journal_name) -> MetadataTable::failed(... METADATA_JOURNAL_TABLE_EXISTS_CODE ...)) is therefore only ever reachable from a DIFFERENT bucket whose name folds to the same table name, never from a delete-then-recreate of the same bucket. On replay the identical create after the delete settled at TableStatus="ACTIVE" with Error=None, where AWS reports TableStatus="FAILED" with ErrorCode="JournalTableAlreadyExists".

Why: a client relying on AWS's behavior gets the opposite verdict about whether its metadata tables actually exist. Real callers that delete a metadata configuration and re-create it must first delete the leftover journal table through s3tables; against the emulator the re-create silently appears healthy, so cleanup code that drops the journal namespace is never exercised and failure-handling for the JournalTableAlreadyExists provisioning error is never reached. It also makes the emulator's managed-namespace model inconsistent: the table name is reported as both released (by the delete) and as owned by a live table (by AWS), so a caller that enumerates the AWS-managed table bucket namespace and a caller that reads TableStatus disagree.

Fix: the state transition behind DeleteBucketMetadataConfiguration — the metadata-configuration delete in the s3 service state (ComAmazonawsS3Amazons3State::delete_metadata_configuration, reached from the DeleteBucketMetadataConfiguration handler) must drop only the bucket's configuration record and LEAVE the managed-namespace entries for the tables it provisioned, so CreateBucketMetadataConfiguration's existing name-taken branch fires and reports the FAILED journal table with its documented ErrorCode and message. Releasing a namespace name belongs with deleting the table itself (the s3tables DeleteTable path), not with deleting the configuration.

Not covered by the open issue 20261027T001031Z-s3-deletebucketmetadataconfiguration-stub.md (that one is about the operation being an unimplemented scaffold stub; it is implemented on this replay and answers the delete successfully), nor by 20261028T141517Z-s3-updatebucketmetadataannotationtableconfiguration-is-an-unimplemented-scaffold-stub-501.md (a different operation).

Traces: s3.create-bucket-metadata-table-configuration-name-conflict-rejected.

## Evidence
Check s3.create-bucket-metadata-table-configuration-name-conflict-rejected [mismatch].

The check's earlier assertions PASSED: first CreateBucketMetadataConfiguration -> journal TableStatus ACTIVE; second identical create while ACTIVE -> ClientError ("MetadataConfigurationAlreadyExists", "The metadata configuration already exists"); GetBucketMetadataConfiguration still ACTIVE.

It then failed on the post-delete re-create:

  File "check_s3_create_bucket_metadata_table_configuration_name_conflict_rejected.py", line 71, in run
    assert restored_status == "FAILED", (
  AssertionError: expected the identical create to still conflict on the leftover journal table, got status=ACTIVE error=None

Expected (AWS), read from GetBucketMetadataConfigurationResult.MetadataConfigurationResult.JournalTableConfigurationResult:
  TableStatus: "FAILED"
  Error: {"ErrorCode": "JournalTableAlreadyExists",
          "ErrorMessage": "A journal table already exists in the AWS managed table bucket's namespace. Delete the journal table, and then try again. To create a new S3 Metadata table, you must delete the S3 Metadata configuration for this bucket, and then create a new S3 Metadata configuration."}

Actual (emulator):
  TableStatus: "ACTIVE"
  Error: absent (None)

Smithy grounding: ErrorDetails$ErrorCode documentation in services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json lists JournalTableAlreadyExists for V2 CreateBucketMetadataConfiguration provisioning failures.

Emulator source grounding:
- services/service-com-amazonaws-s3-amazons3/src/ops/delete_bucket_metadata_configuration.rs:194-218 — the delete releases the configuration AND its namespace table names in one step ("their names are free again and a fresh CreateBucketMetadataConfiguration on the same bucket provisions its tables rather than finding their names taken").
- services/service-com-amazonaws-s3-amazons3/src/ops/create_bucket_metadata_configuration.rs:372-384, 459-498 — the FAILED/JournalTableAlreadyExists branch keyed on store.metadata_namespace_holds(&journal_name), and the namespace recording that the delete above undoes.

## Steps to Reproduce
1. CreateBucket on a general purpose bucket B (with CreateBucketConfiguration.LocationConstraint when the region is not us-east-1).
2. CreateBucketMetadataConfiguration(Bucket=B, MetadataConfiguration={"JournalTableConfiguration": {"RecordExpiration": {"Expiration": "DISABLED"}}}).
3. Poll GetBucketMetadataConfiguration(Bucket=B) until MetadataConfigurationResult.JournalTableConfigurationResult.TableStatus is ACTIVE.
4. DeleteBucketMetadataConfiguration(Bucket=B). Do NOT delete the journal table in the AWS-managed table bucket namespace (arn:aws:s3tables:<region>:<account>:bucket/aws-s3, namespace b_<B>).
5. CreateBucketMetadataConfiguration(Bucket=B, ...) again with the identical MetadataConfiguration. It is accepted synchronously.
6. Poll GetBucketMetadataConfiguration(Bucket=B) for the journal table's terminal status.
   Expected: TableStatus "FAILED" with ErrorDetails ErrorCode "JournalTableAlreadyExists" and the documented ErrorMessage.
   Observed: TableStatus "ACTIVE" and no ErrorDetails.
