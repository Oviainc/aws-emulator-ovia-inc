Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.server-side-encryption-enforced-on-new-tables
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.server-side-encryption-enforced-on-new-tables|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Commit: 0f63d262faa7844f674f0bca27d874696ea1014b

# Glue Data Catalog SSE-KMS is an inert stored flag: PutDataCatalogEncryptionSettings records the CMK but no catalog read ever depends on it, so GetTable never answers GlueEncryptionException when the CMK is disabled

> _Note: this issue was identified against the code base at commit `0f63d262faa7844f674f0bca27d874696ea1014b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTable

## Description
One root cause.

Cause 1 of 1: Catalog encryption-at-rest is persisted as a configuration record only — it is never wired to KMS — so a catalog read does not depend on the customer managed key the catalog claims to be encrypted under. Failing check: glue.server-side-encryption-enforced-on-new-tables.

AWS: Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json) models PutDataCatalogEncryptionSettings with a required DataCatalogEncryptionSettings whose EncryptionAtRest carries a required CatalogEncryptionMode enum and an SseAwsKmsKeyId (NameString, @length 1..255). The operation's own documentation states: "Sets the security configuration for a specified catalog. After the configuration has been set, the specified encryption is applied to every catalog write thereafter." GetTable declares GlueEncryptionException (errorKind client, HTTP 400), documented as "An encryption operation failed.", with a single Message member. On AWS, once EncryptionAtRest.CatalogEncryptionMode is SSE-KMS with SseAwsKmsKeyId = K, the metadata of objects written afterward is encrypted under K, and every subsequent read must decrypt through K. While K is enabled, GetTable returns the table normally. While K is in KeyState Disabled, Glue's decrypt call to KMS fails and GetTable is refused with HTTP 400 and error code GlueEncryptionException; the Message is a relay of the underlying KMS failure (the KMS DisabledException text, which names the key's ARN and carries a per-request id) — the fixed part of the requirement is the 400 status and the GlueEncryptionException code, the variable part is the relayed KMS message naming the disabled key from the request/state. Re-enabling K must make GetTable succeed again and return the byte-identical metadata (same Name, DatabaseName and StorageDescriptor.Columns) — the objects are made unreadable, not replaced.

Emulator: PutDataCatalogEncryptionSettings is implemented purely as a state write — services/service-com-amazonaws-glue-awsglue/src/ops/put_data_catalog_encryption_settings.rs stores the whole input structure into `catalog.encryption_settings` and returns an empty output. It never resolves SseAwsKmsKeyId against KMS, so a key id that does not exist, is disabled, or lives in another region is accepted as-is. GetDataCatalogEncryptionSettings then reads that record back faithfully, which is why the check's first poll (CatalogEncryptionMode == "SSE-KMS", SseAwsKmsKeyId == the created key) passes. GetTable, however, has no encryption path at all: services/service-com-amazonaws-glue-awsglue/src/ops/get_table.rs resolves the catalog id, looks the database and table up in the in-memory store and returns `entry.to_table(...)` — `encryption_settings` is never consulted and no KMS operation is performed, even though GetTableErrorModeled already declares a GlueEncryptionException variant that is never constructed. Consequently, after kms.disable_key(KeyId) succeeds and DescribeKey reports KeyState "Disabled", GetTable on a table created under SSE-KMS keeps returning HTTP 200 with the full table. The check polls for any ClientError for up to 120 s and never observes one, so the check is killed by the harness cap ("Check timed out after 60.0s") rather than reaching its assertion — the timeout is the symptom of a read that never got refused.

Why: a client cannot use this emulator to validate anything that depends on catalog encryption actually being cryptography. Code that proves data-at-rest controls work — a key-rotation or key-revocation runbook, an incident drill that disables a CMK to cut off access, a compliance test asserting that revoking the CMK denies metadata reads, or error handling that catches GlueEncryptionException and surfaces "catalog key unavailable" — all pass against the emulator and then behave differently on AWS. The inverse is just as damaging: an operator rehearsing a CMK disable sees the catalog still fully readable and concludes revocation is ineffective. Because PutDataCatalogEncryptionSettings also accepts a nonexistent or disabled SseAwsKmsKeyId without error, a misconfigured catalog looks healthy in the emulator and fails only in production. Finally, GlueEncryptionException is unreachable across the catalog surface, so no client retry/alert path keyed on that code is ever exercised.

Fix: this belongs in the Glue catalog state plus the read path of the catalog operations. The catalog store (services/service-com-amazonaws-glue-awsglue/src/state.rs) must record, per catalog object written while EncryptionAtRest.CatalogEncryptionMode is SSE-KMS, the key the write was encrypted under, and the catalog read handlers (get_table.rs and its siblings — GetTable, GetTables, GetDatabase/GetDatabases, GetPartition(s)) must consult that key's current state through the emulator's KMS service before answering, returning GetTableErrorModeled::GlueEncryptionException (HTTP 400) with the relayed KMS message when the key is not usable and the unchanged metadata when it is. put_data_catalog_encryption_settings.rs additionally needs validation that SseAwsKmsKeyId resolves to a key in this account/region, answering the modeled InvalidInputException when it does not. The existing-but-unconstructed GlueEncryptionException variant in get_table.rs is the seam.

No existing open issue covers catalog encryption for Glue; the nearest filed items are about other services' KMS wiring (20261025T065401Z-sqs-encryption-is-inert-a-new-queue-reports.md, 20261024T182018Z-ecr-kms-ecr-createrepository-with-encryptiontype-kms-creates-no-kms.md, 20261024T055421Z-kms-opensearch-createdomain-does-not-validate-encryptionatrestoptions-kmskeyid-disabled-and.md) and none of them touch the Glue Data Catalog.

Traces: glue.server-side-encryption-enforced-on-new-tables (verdict mismatch; "Check timed out after 60.0s"). Handler citations: services/service-com-amazonaws-glue-awsglue/src/ops/put_data_catalog_encryption_settings.rs lines 139-199 (stores the settings structure, no KMS resolution); services/service-com-amazonaws-glue-awsglue/src/ops/get_table.rs lines 254-290 (read path returns the stored table with no encryption check) and line 128 (GlueEncryptionException variant declared but never constructed).

## Evidence
Check: glue.server-side-encryption-enforced-on-new-tables (SERVICES = ["glue"], tags axis:happy-path, cap:c1, concern:representation, plane:control, resource:account-singleton:glue-catalog-encryption).

Sequence exercised:
1. kms.create_key -> KeyId K.
2. glue.get_data_catalog_encryption_settings -> saved as the original, restored on cleanup.
3. glue.put_data_catalog_encryption_settings(DataCatalogEncryptionSettings={"EncryptionAtRest": {"CatalogEncryptionMode": "SSE-KMS", "SseAwsKmsKeyId": K}}) -> accepted (HTTP 200).
4. glue.get_data_catalog_encryption_settings -> EncryptionAtRest.CatalogEncryptionMode == "SSE-KMS", SseAwsKmsKeyId == K. PASSES.
5. glue.create_database + glue.create_table (EXTERNAL_TABLE, StorageDescriptor.Columns=[{id,bigint}], no encryption parameter on the request).
6. glue.get_table -> HTTP 200, correct Name/DatabaseName. PASSES (positive control while K is enabled).
7. kms.disable_key(KeyId=K); kms.describe_key -> KeyMetadata.KeyState == "Disabled". PASSES.
8. glue.get_table(DatabaseName, Name) polled every 3 s for up to 120 s, expecting a refusal.

Expected (AWS): step 8's first or an early poll is refused with HTTP 400 and Error.Code == "GlueEncryptionException" (model doc: "An encryption operation failed."), Error.Message being the relayed KMS disabled-key failure naming key K. After kms.enable_key(K), GetTable succeeds again and returns the identical Name, DatabaseName and StorageDescriptor.Columns.

Actual (emulator): step 8 never produces any error. GetTable keeps answering HTTP 200 with the full table while K is Disabled, because get_table.rs reads only the in-memory catalog entry and performs no KMS operation. The check's 120 s denial poll therefore never satisfies its predicate and the run is cut off by the harness: "Error: Check timed out after 60.0s". The check's own assertion message states the diagnosis: "GetTable still succeeded ... after the catalog's CMK was disabled -- the table's metadata is not actually encrypted with that key, so catalog SSE-KMS is a stored flag rather than cryptography". Error code observed: none (no error at all) vs required "GlueEncryptionException".

Static corroboration in the checkout:
- put_data_catalog_encryption_settings.rs: `catalog.encryption_settings = Some(input.data_catalog_encryption_settings);` is the entire effect; SseAwsKmsKeyId is never resolved or validated.
- get_table.rs handle(): after the TransactionId/QueryAsOfTime/IncludeStatusDetails/AttributesToGet guards it does `store.database(...)` -> `database.tables.get(...)` -> `Ok(GetTableOutput { table: Some(entry.to_table(...)) })`. No reference to `encryption_settings` and no KMS call.
- get_table.rs declares `GetTableErrorModeled::GlueEncryptionException(GlueEncryptionException)` but nothing in the crate constructs it, so the modeled error is unreachable.

Smithy citations: GetTable errors include GlueEncryptionException (errorKind client, httpError 400); GlueEncryptionException doc "An encryption operation failed." with member Message (MessageString); EncryptionAtRest.CatalogEncryptionMode required enum, EncryptionAtRest.SseAwsKmsKeyId NameString @length 1..255; PutDataCatalogEncryptionSettings doc "After the configuration has been set, the specified encryption is applied to every catalog write thereafter."

## Steps to Reproduce
1. Boot the emulator from this checkout and point a botocore client at it with both `glue` and `kms` available.
2. key_id = kms.create_key()["KeyMetadata"]["KeyId"]
3. glue.put_data_catalog_encryption_settings(DataCatalogEncryptionSettings={"EncryptionAtRest": {"CatalogEncryptionMode": "SSE-KMS", "SseAwsKmsKeyId": key_id}})
4. glue.get_data_catalog_encryption_settings() — observe EncryptionAtRest.CatalogEncryptionMode == "SSE-KMS" and SseAwsKmsKeyId == key_id (the setting round-trips).
5. db = <unique name>; glue.create_database(DatabaseInput={"Name": db})
6. tbl = <unique name>; glue.create_table(DatabaseName=db, TableInput={"Name": tbl, "TableType": "EXTERNAL_TABLE", "StorageDescriptor": {"Location": f"s3://{db}-nonexistent/data/", "Columns": [{"Name": "id", "Type": "bigint"}]}})
7. glue.get_table(DatabaseName=db, Name=tbl) — succeeds (correct positive control).
8. kms.disable_key(KeyId=key_id); confirm kms.describe_key(KeyId=key_id)["KeyMetadata"]["KeyState"] == "Disabled".
9. Call glue.get_table(DatabaseName=db, Name=tbl) repeatedly for up to 120 s.
   - AWS: refused with HTTP 400, Error.Code == "GlueEncryptionException" and a Message relaying the KMS disabled-key failure for key_id.
   - Emulator: every call returns HTTP 200 with the full table; no error is ever raised, and the behavioral check is killed at the 60 s harness cap.
10. Also reproduce the missing input validation: call step 3 with SseAwsKmsKeyId set to a key id that was never created. AWS refuses the configuration; the emulator stores it and reports it back from GetDataCatalogEncryptionSettings.
11. kms.enable_key(KeyId=key_id) and re-read: on AWS GetTable recovers and returns the identical Name/DatabaseName/StorageDescriptor.Columns (the emulator never diverged here because it never denied the read).
