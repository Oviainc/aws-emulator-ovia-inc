Issue-Source: trace-sweep-divergence
Issue-Trace: glue-database-list-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-database-list-lifecycle glue-database-parameters-map-round-trip glue-database-table-partition-lifecycle glue-resource-policy-and-tagging glue-database-update-and-table glue-database-update-and-table-batch-delete glue-table-versioning-and-column-statistics glue-get-tables-lists-created-tables
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetDatabase|2xx->2xx|$.Database.CatalogIdentifier
Issue-Key: glue|GetDatabase|2xx->2xx|$.Database.CreateTableDefaultPermissions
Issue-Key: glue|GetDatabase|2xx->2xx|$.Database.DatabaseId
Issue-Key: glue|GetTable|2xx->2xx|$.Table.CatalogIdentifier
Issue-Key: glue|GetTable|2xx->2xx|$.Table.CreatedBy
Issue-Key: glue|GetTable|2xx->2xx|$.Table.DatabaseId
Issue-Key: glue|GetTable|2xx->2xx|$.Table.IsMaterializedView
Issue-Key: glue|GetTable|2xx->2xx|$.Table.IsMultiDialectView
Issue-Key: glue|GetTable|2xx->2xx|$.Table.IsRowFilteringEnabled
Issue-Key: glue|GetTable|2xx->2xx|$.Table.StorageDescriptor.SortColumns
Issue-Key: glue|GetTable|2xx->2xx|$.Table.VersionId
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[Name="sim2real_probe_glue_sim2real_probe_tbl_00614834"].CatalogIdentifier
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[Name="sim2real_probe_glue_sim2real_probe_tbl_00614834"].CreatedBy
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[Name="sim2real_probe_glue_sim2real_probe_tbl_00614834"].DatabaseId
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[Name="sim2real_probe_glue_sim2real_probe_tbl_00614834"].IsMaterializedView
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[Name="sim2real_probe_glue_sim2real_probe_tbl_00614834"].IsMultiDialectView
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[Name="sim2real_probe_glue_sim2real_probe_tbl_00614834"].IsRowFilteringEnabled
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[Name="sim2real_probe_glue_sim2real_probe_tbl_00614834"].StorageDescriptor.SortColumns
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[Name="sim2real_probe_glue_sim2real_probe_tbl_00614834"].VersionId
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*].CatalogIdentifier
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*].CreatedBy
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*].DatabaseId
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*].IsMaterializedView
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*].IsMultiDialectView
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*].IsRowFilteringEnabled
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*].StorageDescriptor.SortColumns
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*].VersionId
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# Glue Table and Database projections omit modeled catalog metadata fields

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTable

## Description
Five wire-fixture clusters, all one family of defect: the Glue emulator's Database and Table response projections drop members that real Glue always returns, and fill two more with values of its own invention. Every step below answered HTTP 200 with the right status and the right entity, so this is purely the shape of the serialized entity. Each cause must be closed before this issue is done. (The open behavioral issue 20261022T131025Z-glue-createjob-is-a-501-stub-gettable-ignores.md names the Table.CreatedBy symptom under its Cause 3, but it carries no wire replay key for these fixture steps and does not cover the omitted members, so these causes are filed here.)

Cause 1 of 5: GetDatabase omits CatalogIdentifier, DatabaseId and CreateTableDefaultPermissions.
AWS: Smithy com.amazonaws.glue#GetDatabase -> output member Database (com.amazonaws.glue#Database). Database.CreateTableDefaultPermissions is modeled (target PrincipalPermissionsList of PrincipalPermissions{Principal: DataLakePrincipal, Permissions: PermissionList}) and real Glue populates it for a database created without an explicit value with the single IAM_ALLOWED_PRINCIPALS/ALL grant. CatalogIdentifier and DatabaseId are recorded behavior only - they are not declared on com.amazonaws.glue#Database, but the recording shows AWS returning both on every GetDatabase: CatalogIdentifier is the catalog's account id and DatabaseId is an opaque 32-hex-character catalog id, stable for the life of the database.
Emulator: all three members are absent from the serialized Database object (replay reports them as `missing`).
Why: a client that reads CreateTableDefaultPermissions to decide whether a database is Lake-Formation-governed or IAM_ALLOWED_PRINCIPALS-open sees nothing at all, which is indistinguishable from an explicitly empty grant list - the opposite governance decision. A client that keys its own records off Database.DatabaseId gets None and has no stable handle for the database.
Fix: the Database projection shared by the GetDatabase/GetDatabases handlers and the database state record (services/service-com-amazonaws-glue-awsglue/src/ops/get_database.rs and the Database projection it serializes) must emit the catalog account id, a database-creation-time opaque id, and the default CreateTableDefaultPermissions grant when CreateDatabase/UpdateDatabase did not set one. The id must be generated once at create time and persisted, not derived per request, so repeated reads agree.
Traces: glue-database-list-lifecycle#task#4 (GetDatabase, cluster also covers glue-database-parameters-map-round-trip#task#2, glue-database-table-partition-lifecycle#task#5, glue-resource-policy-and-tagging#setup#4).

Cause 2 of 5: the same GetDatabase omission on a second database shape (CatalogIdentifier and CreateTableDefaultPermissions missing), confirming it is the projection and not one fixture's input.
AWS: as Cause 1.
Emulator: GetDatabase returns the database with CatalogIdentifier and CreateTableDefaultPermissions absent.
Why: as Cause 1.
Fix: as Cause 1 - one projection change closes both, but both clusters must be re-replayed.
Traces: glue-database-update-and-table#setup#5 (cluster also covers glue-database-update-and-table-batch-delete#setup#5, glue-table-versioning-and-column-statistics#setup#4).

Cause 3 of 5: GetTable omits six Table members and reports a wrong VersionId and a synthesized CreatedBy.
AWS: Smithy com.amazonaws.glue#GetTable -> output member Table (com.amazonaws.glue#Table). Modeled members the recording shows populated and the emulator drops: IsMultiDialectView and IsMaterializedView (NullableBoolean, returned false for a plain EXTERNAL_TABLE) and StorageDescriptor.SortColumns (returned as an empty list, not omitted). VersionId (VersionString, "The ID of the table version") is '0' for a table that has never been updated. CreatedBy (NameString, "The person or entity who created the table") is the creating caller's principal ARN, which for this corpus is arn:aws:iam::000000000000:user/emulator - the same identity STS reports for the session. CatalogIdentifier, DatabaseId and IsRowFilteringEnabled are recorded behavior only - not declared on com.amazonaws.glue#Table, but returned by AWS on every GetTable (account id, the owning database's opaque id, and false).
Emulator: CatalogIdentifier, DatabaseId, IsMaterializedView, IsMultiDialectView, IsRowFilteringEnabled and StorageDescriptor.SortColumns are all absent; VersionId is '1' on a freshly created table; CreatedBy is a per-run synthesized principal of the form arn:aws:iam::000000000000:user/EMULATOR<hex> instead of the session's caller ARN.
Why: a client that branches on IsMaterializedView/IsMultiDialectView gets None where AWS gives an explicit false, so three-state logic fires the wrong branch; a client that iterates StorageDescriptor.SortColumns crashes on None rather than looping zero times; version-aware code that fetches table version '0' after CreateTable gets a 404-equivalent because the emulator numbered the first version '1'; and ownership/audit code correlating CreatedBy with the authenticated identity sees a principal that exists nowhere else in the account.
Fix: the Table projection used by get_table.rs (services/service-com-amazonaws-glue-awsglue/src/ops/get_table.rs and the shared Table serializer) must emit the catalog account id, the owning database id, the three boolean members as explicit false, SortColumns as an empty list rather than omitted; CreateTable must number the initial table version '0'; and CreatedBy must be taken from the request's resolved caller principal ARN rather than generated.
Traces: glue-database-table-partition-lifecycle#task#6.

Cause 4 of 5: GetTables carries the same per-table omissions into every element of TableList.
AWS: Smithy com.amazonaws.glue#GetTables -> output TableList (list of com.amazonaws.glue#Table); each element is the same projection GetTable returns, so the members in Cause 3 are present on each element.
Emulator: every TableList element is missing CatalogIdentifier, DatabaseId, IsMaterializedView, IsMultiDialectView, IsRowFilteringEnabled and StorageDescriptor.SortColumns, and carries VersionId '1' and the synthesized EMULATOR<hex> CreatedBy.
Why: the list read is the path most clients use to enumerate a database, so the defect in Cause 3 reaches clients even when they never call GetTable, and it is per-element rather than one bad field.
Fix: as Cause 3 - GetTables must serialize each table through the same corrected Table projection, not a second reduced one.
Traces: glue-database-table-partition-lifecycle#task#7.

Cause 5 of 5: GetTables on a two-table database shows the omissions on both elements (TableList[0] and TableList[1]).
AWS: as Cause 4.
Emulator: both elements omit the six members and report VersionId '1' plus the synthesized CreatedBy.
Why: as Cause 4; this cluster also shows the defect is not an artifact of a single-element list.
Fix: as Cause 4.
Traces: glue-get-tables-lists-created-tables#task#7.

## Evidence
Replay of simparity-wire-aws-v1 (121 fixtures, 898 steps, state_reset_failures 0) - all five clusters are verdict=mismatch with expected_status 200 and actual_status 200, so only the response body diverges.

Cause 1 - glue-database-list-lifecycle#task#4 GetDatabase: expected $.Database.CatalogIdentifier '000000000000', $.Database.DatabaseId (32-hex, e.g. '15694831796e4b2fa91b599899a75344'), $.Database.CreateTableDefaultPermissions [{Principal:{DataLakePrincipalIdentifier:'IAM_ALLOWED_PRINCIPALS'},Permissions:['ALL']}]. Actual: all three missing (None).

Cause 2 - glue-database-update-and-table#setup#5 GetDatabase: expected $.Database.CatalogIdentifier '000000000000' and $.Database.CreateTableDefaultPermissions as above. Actual: both missing.

Cause 3 - glue-database-table-partition-lifecycle#task#6 GetTable: expected $.Table.CatalogIdentifier '000000000000', $.Table.DatabaseId set, $.Table.IsMaterializedView false, $.Table.IsMultiDialectView false, $.Table.IsRowFilteringEnabled false, $.Table.StorageDescriptor.SortColumns [], $.Table.VersionId '0', $.Table.CreatedBy 'arn:aws:iam::000000000000:user/emulator'. Actual: the first six missing, VersionId '1', CreatedBy 'arn:aws:iam::000000000000:user/EMULATOR84AE1286B56B' (the EMULATOR<hex> suffix is per-run; the defect is that it is synthesized, not the literal value).

Cause 4 - glue-database-table-partition-lifecycle#task#7 GetTables: same eight divergences on TableList[Name="sim2real_probe_glue_sim2real_probe_tbl_00614834"].

Cause 5 - glue-get-tables-lists-created-tables#task#7 GetTables: same eight divergences on both TableList[0] and TableList[1] (CreatedBy there 'arn:aws:iam::000000000000:user/EMULATORBEF86533C607').

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay fixture glue-database-list-lifecycle, step task#4 - CreateDatabase with no CreateTableDefaultPermissions, then GetDatabase and inspect $.Database for CatalogIdentifier/DatabaseId/CreateTableDefaultPermissions.
3. Cause 2: replay fixture glue-database-update-and-table, step setup#5 (same GetDatabase read on the setup database).
4. Cause 3: replay fixture glue-database-table-partition-lifecycle, step task#6 - CreateTable in that database, then GetTable and inspect $.Table for the six omitted members, VersionId and CreatedBy.
5. Cause 4: same fixture, step task#7 - GetTables on that database and inspect the TableList element.
6. Cause 5: replay fixture glue-get-tables-lists-created-tables, step task#7 - GetTables on a database holding two tables and inspect TableList[0] and TableList[1].
