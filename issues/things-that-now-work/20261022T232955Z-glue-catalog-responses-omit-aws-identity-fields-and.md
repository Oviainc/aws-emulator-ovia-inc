Issue-Source: trace-sweep-divergence
Issue-Trace: glue-database-list-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-database-list-lifecycle glue-database-parameters-map-round-trip glue-database-table-partition-lifecycle glue-database-update-and-table glue-database-update-and-table-batch-delete glue-resource-policy-and-tagging glue-table-versioning-and-column-statistics glue-get-tables-lists-created-tables
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetDatabase|2xx->2xx|$.Database.CatalogIdentifier
Issue-Key: glue|GetDatabase|2xx->2xx|$.Database.CreateTableDefaultPermissions[*].__type
Issue-Key: glue|GetDatabase|2xx->2xx|$.Database.DatabaseId
Issue-Key: glue|GetTable|2xx->2xx|$.Table.CatalogIdentifier
Issue-Key: glue|GetTable|2xx->2xx|$.Table.DatabaseId
Issue-Key: glue|GetTable|2xx->2xx|$.Table.IsRowFilteringEnabled
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[Name="sim2real_probe_glue_sim2real_probe_tbl_00614834"].CatalogIdentifier
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[Name="sim2real_probe_glue_sim2real_probe_tbl_00614834"].DatabaseId
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[Name="sim2real_probe_glue_sim2real_probe_tbl_00614834"].IsRowFilteringEnabled
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*].CatalogIdentifier
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*].DatabaseId
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*].IsRowFilteringEnabled
Issue-Key: glue|BatchDeleteTable|2xx->2xx|$.Errors[*].ErrorDetail.ErrorMessage
Issue-Key: glue|BatchDeleteTable|2xx->2xx|$.Errors[*].ErrorDetail.__type
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Glue catalog responses omit AWS identity fields and nested __type discriminators

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDatabase

## Description
Six wire-fixture clusters in which the Glue emulator answers with the right HTTP status (200 in every case) but its serialized Data Catalog payload is missing members the recording shows AWS emitting: the per-entity identity/attribution fields (CatalogIdentifier, DatabaseId, IsRowFilteringEnabled) and the `__type` discriminator AWS writes onto nested structures in its awsJson1.1 bodies (PrincipalPermissions inside Database.CreateTableDefaultPermissions, ErrorDetail inside BatchDeleteTable.Errors). These are all one defect family -- the Database/Table projection and the nested-structure serializer used by every catalog read -- but each cluster is its own cause and each must be closed before this issue is done. Note that none of these six clusters is covered by the already-open issue 20261022T192452Z-glue-error-bodies-use-emulator-authored-message-templates.md (that one is about $.Message text on 4xx bodies) nor by 20261022T131025Z-glue-createjob-is-a-501-stub-gettable-ignores.md (CreateJob stub, Lake Formation gating, CreatedBy, CFN provisioning).

Cause 1 of 6: GetDatabase omits CatalogIdentifier, DatabaseId and the nested PrincipalPermissions __type.
AWS: recorded behavior only -- Smithy com.amazonaws.glue#Database models Name, Description, LocationUri, Parameters, CreateTime, CreateTableDefaultPermissions (PrincipalPermissionsList), TargetDatabase, CatalogId and FederatedDatabase, and does NOT model CatalogIdentifier, DatabaseId or a __type member; the recording nevertheless shows the live service returning, inside $.Database, a CatalogIdentifier (the account/catalog id owning the database, '000000000000' in this environment), a DatabaseId (an opaque 32-hex-character catalog-assigned id for the database) and a `__type` of 'PrincipalPermissions' on each element of CreateTableDefaultPermissions. The requirement is that those members are present and well-formed, not the particular generated id in the recording.
Emulator: 200 with $.Database.CatalogIdentifier, $.Database.DatabaseId and $.Database.CreateTableDefaultPermissions[0].__type all absent (None).
Why: a client that reads the catalog id or the database id off a GetDatabase response -- to build an ARN, to correlate the database across Lake Formation/Athena calls, or to key its own records on the catalog-assigned id -- gets null where AWS gives a value, and a deserializer that dispatches on the nested `__type` tag cannot identify the permission entries.
Fix: the Database projection shared by the GetDatabase handler and its serializer (services/service-com-amazonaws-glue-awsglue/src/ops/get_database.rs plus the Database serialization used by the catalog read paths) must populate CatalogIdentifier from the request's resolved catalog/account id, assign and persist a stable DatabaseId at CreateDatabase time in the service state so repeated reads return the same id, and emit the __type tag on nested PrincipalPermissions structures.
Traces: glue-database-list-lifecycle#task#4 (cluster also covers glue-database-parameters-map-round-trip#task#2, glue-database-table-partition-lifecycle#task#5, glue-database-update-and-table#task#7, glue-database-update-and-table-batch-delete#task#7, glue-resource-policy-and-tagging#setup#4).

Cause 2 of 6: GetDatabase omits CatalogIdentifier and the nested PrincipalPermissions __type on freshly created databases.
AWS: recorded behavior only, as Cause 1 -- $.Database carries CatalogIdentifier ('000000000000' here) and a `__type` of 'PrincipalPermissions' on each CreateTableDefaultPermissions element immediately after CreateDatabase, i.e. the fields are part of the standing projection and not something that appears only after a later update.
Emulator: 200 with both members absent.
Why: the omission is not confined to one fixture's lifecycle stage -- setup-time reads are equally unusable for ARN construction and for discriminator-driven deserialization. This cluster is reported separately from Cause 1 because it is a distinct replay key (no DatabaseId assertion) and must be re-replayed on its own.
Fix: same component as Cause 1 -- make the fields part of the Database projection rather than of any particular code path.
Traces: glue-database-update-and-table#setup#5 (cluster also covers glue-database-update-and-table-batch-delete#setup#5, glue-table-versioning-and-column-statistics#setup#4).

Cause 3 of 6: GetTable omits CatalogIdentifier, DatabaseId and IsRowFilteringEnabled.
AWS: recorded behavior only -- Smithy com.amazonaws.glue#Table models Name, DatabaseName, CatalogId, VersionId, StorageDescriptor, PartitionKeys, IsRegisteredWithLakeFormation, RLSProperties and so on, and does NOT model CatalogIdentifier, DatabaseId or IsRowFilteringEnabled; the recording shows the live service returning all three inside $.Table: CatalogIdentifier (the owning catalog/account id), DatabaseId (the containing database's catalog-assigned id -- the fixture binds it to the same value GetDatabase returned, so the two responses must agree), and IsRowFilteringEnabled as a boolean that is false for a table with no row-level filter.
Emulator: 200 with all three members absent (None).
Why: a client cannot tell from the emulator whether row filtering is on (absent is not false for a consumer that distinguishes them), and loses the catalog/database identity that ties the table back to the database it read a moment earlier -- the cross-response agreement on DatabaseId is exactly what the recording pins down.
Fix: the Table projection used by the table read paths (services/service-com-amazonaws-glue-awsglue/src/ops/get_table.rs and the shared Table serializer) must emit CatalogIdentifier, the containing database's DatabaseId from the stored catalog state, and IsRowFilteringEnabled defaulting to false.
Traces: glue-database-table-partition-lifecycle#task#6 (cluster also covers glue-database-table-partition-lifecycle#task#10).

Cause 4 of 6: GetTables omits the same three Table members on the listed tables.
AWS: recorded behavior only -- each element of $.TableList carries CatalogIdentifier, DatabaseId and IsRowFilteringEnabled exactly as the single-table read does, i.e. the list projection is the same projection.
Emulator: 200 with CatalogIdentifier, DatabaseId and IsRowFilteringEnabled absent on the listed table.
Why: a client that enumerates a database and then works from the listing (rather than re-reading each table) never sees the identity or the row-filtering flag at all, so list-then-act flows are strictly less informative than against AWS.
Fix: route GetTables through the same Table projection fixed in Cause 3 instead of a separate list-only shaping (services/service-com-amazonaws-glue-awsglue/src/ops/get_tables.rs).
Traces: glue-database-table-partition-lifecycle#task#7.

Cause 5 of 6: GetTables omits the same three members on every element of a multi-table listing.
AWS: recorded behavior only, as Cause 4 -- both listed tables carry CatalogIdentifier, DatabaseId (identical for tables in the same database) and IsRowFilteringEnabled false.
Emulator: 200 with all three absent on TableList[0] and TableList[1].
Why: confirms the gap is per-element and not an artifact of a single-element list; a client paging a populated database gets a uniformly under-populated projection. Reported separately because it is its own replay key.
Fix: same component as Cause 4.
Traces: glue-get-tables-lists-created-tables#task#7.

Cause 6 of 6: BatchDeleteTable's per-table error entry omits the ErrorDetail __type and carries an emulator-authored ErrorMessage.
AWS: recorded behavior only for the tag -- Smithy com.amazonaws.glue#ErrorDetail models only ErrorCode and ErrorMessage, but the recording shows the live service emitting a `__type` of 'ErrorDetail' on each $.Errors[].ErrorDetail structure. BatchDeleteTable itself answers HTTP 200 and reports a missing table as a per-item error rather than a top-level fault; the recorded ErrorMessage is built from the request's table name, 'Table <Name> not found' -- naming the table and not the database -- so that template is the requirement, not the recorded instance.
Emulator: 200, the per-item error is reported, but $.Errors[0].ErrorDetail.__type is absent and ErrorMessage is 'Table <Name> not found in database <DatabaseName>' -- the emulator's own template, appending the database AWS does not include.
Why: a client that dispatches on the nested `__type` tag cannot identify the error entries, and one that matches AWS's per-item wording to distinguish a missing table from another per-item failure does not match. The open issue 20261022T192452Z-glue-error-bodies-use-emulator-authored-message-templates.md covers the 4xx $.Message templates for GetTable/DeleteTable/GetCrawler/CreateDatabase and does not carry this cluster's replay key, so the BatchDeleteTable per-item template is unfixed by it.
Fix: in the BatchDeleteTable handler (services/service-com-amazonaws-glue-awsglue/src/ops/batch_delete_table.rs) build the per-item ErrorDetail with the AWS template interpolating only the requested table name, and emit the __type tag from the nested-structure serializer fixed in Cause 1.
Traces: glue-database-update-and-table#task#13 (cluster also covers glue-database-update-and-table-batch-delete#task#13).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: 121 fixtures, 898 steps, 174 passed, score 19.4%, state_reset_failures 0, bootstrap_signal false (health 200, data listener accepting). In all six clusters expected_status == actual_status == 200; only body members diverge.

Cause 1 - glue-database-list-lifecycle#task#4 GetDatabase (6 steps affected): $.Database.CatalogIdentifier missing (expected '000000000000', got None); $.Database.CreateTableDefaultPermissions[0].__type missing (expected 'PrincipalPermissions', got None); $.Database.DatabaseId missing (expected a 32-hex catalog id, '15694831796e4b2fa91b599899a75344' in this run, got None).

Cause 2 - glue-database-update-and-table#setup#5 GetDatabase (3 steps affected): $.Database.CatalogIdentifier missing (expected '000000000000', got None); $.Database.CreateTableDefaultPermissions[0].__type missing (expected 'PrincipalPermissions', got None).

Cause 3 - glue-database-table-partition-lifecycle#task#6 GetTable (2 steps affected): $.Table.CatalogIdentifier missing (expected '000000000000', got None); $.Table.DatabaseId missing (expected the DatabaseId the fixture captured from GetDatabase, got None); $.Table.IsRowFilteringEnabled missing (expected False, got None).

Cause 4 - glue-database-table-partition-lifecycle#task#7 GetTables: $.TableList[Name="..."].CatalogIdentifier, .DatabaseId and .IsRowFilteringEnabled all missing (expected '000000000000', the captured database id, and False respectively; got None).

Cause 5 - glue-get-tables-lists-created-tables#task#7 GetTables: the same three members missing on both TableList[0] and TableList[1] (expected '000000000000', '3ba1f7bace404d67918a5087cce1bc83', False).

Cause 6 - glue-database-update-and-table#task#13 BatchDeleteTable (2 steps affected): $.Errors[0].ErrorDetail.__type missing (expected 'ErrorDetail', got None); $.Errors[0].ErrorDetail.ErrorMessage value_mismatch - expected 'Table <tbl> not found' (recorded instance: 'Table sim2real_probe_glue_sim2real_probe_tblgone_38423a92 not found'), actual 'Table sim2real_probe_glue_sim2real_probe_tblgone_38423a92 not found in database sim2real_probe_glue_sim2real_probe_db_38423a92'.

Model evidence: smithy_query on services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json for shapes Database, Table, PrincipalPermissions and ErrorDetail confirms none of CatalogIdentifier, DatabaseId, IsRowFilteringEnabled or __type is a modeled member - hence 'recorded behavior only' for every cause.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay fixture glue-database-list-lifecycle, step task#4 - CreateDatabase then GetDatabase, and check $.Database for CatalogIdentifier, DatabaseId and CreateTableDefaultPermissions[0].__type.
3. Cause 2: replay fixture glue-database-update-and-table, step setup#5 - GetDatabase right after creation, and check $.Database.CatalogIdentifier and CreateTableDefaultPermissions[0].__type.
4. Cause 3: replay fixture glue-database-table-partition-lifecycle, step task#6 - CreateTable then GetTable, and check $.Table.CatalogIdentifier, $.Table.DatabaseId (it must equal the DatabaseId GetDatabase returned for the containing database) and $.Table.IsRowFilteringEnabled == false.
5. Cause 4: same fixture, step task#7 - GetTables on that database and check the same three members on the listed table.
6. Cause 5: replay fixture glue-get-tables-lists-created-tables, step task#7 - GetTables on a database holding two tables and check the three members on both elements.
7. Cause 6: replay fixture glue-database-update-and-table, step task#13 - BatchDeleteTable including a table name that does not exist; the call must return 200 with $.Errors[0].ErrorDetail.__type == 'ErrorDetail' and ErrorMessage 'Table <Name> not found' (no database clause).
