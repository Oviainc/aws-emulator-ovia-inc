Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__s3.athena-glue-catalog-missing-permission-negative athena__glue__s3.athena-writes-csv-and-metadata-for-a-select-and-txt-for-ddl athena__glue__s3.dash-fields-in-numeric-columns-read-as-null-and-typed-columns-aggregate athena__glue__s3.glue-accepts-a-location-in-an-absent-bucket-that-athena-rejects-at-query-time athena__glue__s3.glue-accepts-an-invalid-column-type-that-athena-rejects-at-query-time athena__glue__s3.athena-ddl-table-differs-from-the-glue-api-table-in-owner-partitions-and-location
Issue-Key: athena__glue__s3.athena-glue-catalog-missing-permission-negative|run#12|AssertionError|assert state_bad == "FAILED", f"expected FAILED, got {state_bad} (reason={reason_bad})"
Issue-Key: athena__glue__s3.athena-writes-csv-and-metadata-for-a-select-and-txt-for-ddl|run#18|AssertionError|assert keys == want, f"the result prefix holds {keys!r}, expected {want!r}"
Issue-Key: athena__glue__s3.dash-fields-in-numeric-columns-read-as-null-and-typed-columns-aggregate|run#19|AssertionError|assert types == [["date", "bigint", "double", "integer", "bigint"]], (
Issue-Key: athena__glue__s3.glue-accepts-a-location-in-an-absent-bucket-that-athena-rejects-at-query-time|run#16|AssertionError|assert got == ("FAILED", 2, 1306, "HIVE_FILESYSTEM_ERROR"), (
Issue-Key: athena__glue__s3.glue-accepts-an-invalid-column-type-that-athena-rejects-at-query-time|run#17|AssertionError|assert got == ("FAILED", 2, 1100, "HIVE_INVALID_METADATA"), (
Issue-Key: athena__glue__s3.athena-ddl-table-differs-from-the-glue-api-table-in-owner-partitions-and-location|run#17|AssertionError|assert ddl_view == {
Issue-Commit: 2d22b922898c4268bec96b0923119d5f2b2856eb

# athena__glue__s3: Athena queries bypass Glue authorization, write no result object, and misreport catalog-driven failures and column types (6 causes)

> _Note: this issue was identified against the code base at commit `2d22b922898c4268bec96b0923119d5f2b2856eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
SIX net-new causes under the athena__glue__s3 heading. The Athena data plane now reads Glue-catalogued CSV data correctly (every one of these checks gets its positive control and its aggregate values right), so what remains are six distinct defects in authorization, result materialization, failure classification, type reporting and the Hive-compatibility shape of a DDL-created catalog entry.

Cause 1 of 6: a query runs to SUCCEEDED for a caller that holds no Glue permissions at all.
AWS: Athena calls Glue with the CALLER's identity. A principal with Athena and S3 permissions but no glue:* settles the execution State 'FAILED' with StateChangeReason of the fixed form "Insufficient permissions to execute the query. User: <principal-arn> is not authorized to perform: glue:GetDatabase on resource: arn:aws:glue:<region>:<account-id>:catalog because no identity-based policy allows the glue:GetDatabase action " — 'Insufficient permissions to execute the query. User: ', ' is not authorized to perform: glue:GetDatabase on resource: ', ' because no identity-based policy allows the glue:GetDatabase action ' and the trailing space are fixed text; the assumed-role session ARN, region and account id are taken from the caller and the environment. GetQueryResults on that execution then raises InvalidRequestException with the fixed message "Query did not finish successfully. Final query state: FAILED".
Emulator: the denied caller's 'SELECT * FROM <db>.<table>' reaches State 'SUCCEEDED' with StateChangeReason None and serves the row, so no refusal and no result-set error ever occur.
Why: the emulator answers a query the real service denies, so an IAM least-privilege boundary verified against it is not a boundary at all, and the data of a catalog table is disclosed to a principal with no catalog permission. Note the open issue 20261104T163600Z-athena-does-not-authorize-startqueryexecution-against-the-caller.md covers workgroup-scoped athena: authorization; this cause is the glue: authorization Athena performs on the caller's behalf and is not carried by it.
Fix: the StartQueryExecution/query-engine catalog-access path — evaluate the caller's identity-based policy for the Glue actions the plan needs and settle the execution FAILED with the message above when they are denied.
Traces: athena__glue__s3.athena-glue-catalog-missing-permission-negative.

Cause 2 of 6: no result object is written under the output location, for either a SELECT or a DDL statement.
AWS: a successful DML statement writes exactly two objects under the workgroup's result prefix, '<QueryExecutionId>.csv' (the result set, header row first, every field double-quoted) and '<QueryExecutionId>.csv.metadata'; a DDL statement writes exactly one object, '<QueryExecutionId>.txt'. The names are derived from the execution id and the configured prefix; nothing else appears in the prefix.
Emulator: GetQueryExecution reports the right ResultConfiguration.OutputLocation for both statements ('s3://<bucket>/athena-results/<id>.csv' and '.../<id>.txt') and the right StatementType, but listing the prefix afterwards returns [] — no .csv, no .csv.metadata, no .txt. Open issues 20261104T163600Z-athena-writes-no-query-result-object-to-s3.md, 20261106T181113Z-athena-never-writes-the-query-result-object-to.md and 20261108T043811Z-athena-s3-athena-reports-a-query-succeeded-and-names-a.md describe the same family under other service headings but do not carry this check's replay key, and none of them covers the DDL '.txt' object or the '.csv.metadata' sidecar.
Why: every consumer that reads results from S3 instead of GetQueryResults (ETL that copies the CSV, a crawler over the result prefix, a client that checks the .metadata sidecar, a DDL audit that reads the .txt) finds nothing at a location the service told it to use, while the API claims success.
Fix: the query-execution result-materialization step — write the CSV and its .metadata sidecar for DML and the .txt for DDL to the resolved output location.
Traces: athena__glue__s3.athena-writes-csv-and-metadata-for-a-select-and-txt-for-ddl.

Cause 3 of 6: typeof() reports the engine's per-value storage class instead of the column's declared type.
AWS: a column's type in a result is the type declared for it in the catalog, independent of the values stored in the files; typeof() over the CloudFront-log schema returns 'date', 'bigint', 'double', 'integer', 'bigint' for columns declared date/bigint/double/int/bigint, including for a bigint column whose every field is CloudFront's '-' (those read as NULL but the column stays bigint).
Emulator: the same query returns ['text', 'integer', 'real', 'integer', 'null'] — SQLite storage-class names, and 'null' for the all-'-' bigint column, i.e. the type is inferred per value from the backing engine rather than taken from the Glue column definition. (The aggregate assertions in the same check pass, so the data read itself is right.) Open issues 20261104T163600Z-athena-statements-are-executed-by-a-sqlite-engine.md and 20261108T041854Z-athena-getqueryresults-omits-columninfo-catalog-schema-table-updatecount.md describe neighbouring type-mapping defects but do not carry this check's replay key.
Why: any client that introspects types — a BI tool mapping ColumnInfo.Type (Smithy ColumnInfo.Type is required) to its own types, a migration that asserts a schema, SQL that branches on typeof() — gets names that are not Athena/Trino type names at all, and a column reported as 'null' has no type to map.
Fix: the query engine's type resolution/serialization — carry the catalog's declared Hive type through planning and expose it in typeof() and ColumnInfo.Type instead of the backing engine's storage class.
Traces: athena__glue__s3.dash-fields-in-numeric-columns-read-as-null-and-typed-columns-aggregate.

Cause 4 of 6: a query over a table whose LOCATION is in a nonexistent bucket fails with the wrong classification and message.
AWS: the catalog never dereferences a table's LOCATION, so Glue accepts it; the query then fails at read time with State 'FAILED', AthenaError.ErrorCategory 2 (user error) and AthenaError.ErrorType 1306, and StateChangeReason / ErrorMessage beginning with the fixed prefix 'HIVE_FILESYSTEM_ERROR: ' followed by the filesystem detail naming the inaccessible location (the bucket/prefix from the table definition is the only variable part). Smithy com.amazonaws.athena#AthenaError models ErrorCategory (integer, range 1-3) and ErrorType (integer, range 0-9999); the category/type values and the prefix are recorded behavior.
Emulator: the execution does end 'FAILED' with ErrorCategory 2, but ErrorType is absent entirely and StateChangeReason is 'Permission denied on S3 path…' — a wrong error class (an access decision, not a filesystem error) with no HIVE_FILESYSTEM_ERROR prefix for a client or log scraper to match. The open issue 20261108T041854Z-athena-query-failures-are-misclassified-athenaerror-errortype-absent.md covers two other query-failure classifications (an object-key LOCATION and an Iceberg table) and does not carry this check's replay key.
Why: ErrorType is the documented code a client maps to a remediation and the HIVE_* prefix is what runbooks and alert rules match; reported as a permission problem, an operator chases IAM for a missing bucket.
Fix: the query engine's failure classification for storage reads — distinguish 'the location cannot be read' from 'access denied' and emit ErrorType 1306 with the HIVE_FILESYSTEM_ERROR message.
Traces: athena__glue__s3.glue-accepts-a-location-in-an-absent-bucket-that-athena-rejects-at-query-time.

Cause 5 of 6: a query over a table with an invalid column type SUCCEEDS instead of failing with HIVE_INVALID_METADATA.
AWS: Glue stores a column typed 'notatype' without complaint, but Athena validates the table's metadata when it plans the read and settles State 'FAILED' with AthenaError.ErrorCategory 2, AthenaError.ErrorType 1100 and StateChangeReason / ErrorMessage beginning with the fixed prefix 'HIVE_INVALID_METADATA: ' and naming the offending column and its unparsable type (the column name and type text are taken from the catalog entry).
Emulator: 'SELECT count(*)' over that table ends State 'SUCCEEDED' with no AthenaError, no ErrorType and an empty StateChangeReason — the invalid type is silently tolerated and a count is returned.
Why: a client validating a catalog definition by querying it is told a table Athena cannot read is readable; the failure only surfaces in the real deployment, and the row counts the emulator returns for it are fabricated agreement.
Fix: the query engine's metadata validation at plan time — reject unparsable Glue column types and settle the execution FAILED with the AthenaError above.
Traces: athena__glue__s3.glue-accepts-an-invalid-column-type-that-athena-rejects-at-query-time.

Cause 6 of 6: Glue GetTable omits PartitionKeys for a table created through Athena DDL.
AWS: a CREATE EXTERNAL TABLE issued through Athena stores a Hive-compatible table whose GetTable response carries PartitionKeys as an EMPTY LIST for an unpartitioned table (alongside Owner 'hadoop', StorageDescriptor.NumberOfBuckets -1, the serialization.format SerDe parameter, transient_lastDdlTime and the location with its trailing slash stripped) — in contrast to a table created through the Glue API, where PartitionKeys is absent. Smithy com.amazonaws.glue#Table.PartitionKeys is a list of Column; the empty-list-vs-absent distinction between the two creation routes is recorded behavior.
Emulator: every other Hive-compatibility default is now correct (Owner 'hadoop', NumberOfBuckets -1, stripped location, serialization.format/field.delim, LazySimpleSerDe, TableType EXTERNAL_TABLE), but PartitionKeys comes back absent (None) rather than []. The open issue 20261108T041854Z-glue-gettables-omits-hive-compatibility-defaults-on-tables.md is about GetTableS and does not carry this check's replay key.
Why: a client that tells the two creation routes apart, or that iterates table['PartitionKeys'] without a default, sees a DDL-created table shaped like an API-created one and either mis-branches or raises a KeyError on a table AWS always returns the member for.
Fix: the Athena-DDL to Glue-catalog write path (and/or Glue's GetTable serializer) — persist and return an empty PartitionKeys list for an unpartitioned table created through DDL.
Traces: athena__glue__s3.athena-ddl-table-differs-from-the-glue-api-table-in-owner-partitions-and-location.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout, service heading 'athena__glue__s3' (13/56 passed). All six checks below passed their positive controls before the assertion shown.

Cause 1 — athena-glue-catalog-missing-permission-negative [mismatch]:
  AssertionError: expected FAILED, got SUCCEEDED (reason=None)
  expected State FAILED with StateChangeReason "Insufficient permissions to execute the query. User: <limited-role-session-arn> is not authorized to perform: glue:GetDatabase on resource: arn:aws:glue:<region>:<account>:catalog because no identity-based policy allows the glue:GetDatabase action " and GetQueryResults -> InvalidRequestException "Query did not finish successfully. Final query state: FAILED".

Cause 2 — athena-writes-csv-and-metadata-for-a-select-and-txt-for-ddl [mismatch]:
  AssertionError: the result prefix holds [], expected ['athena-results/<sel-id>.csv', 'athena-results/<sel-id>.csv.metadata', 'athena-results/<ddl-id>.txt']
  (the preceding assertions on StatementType DDL/DML and on ResultConfiguration.OutputLocation naming <id>.txt / <id>.csv passed.)

Cause 3 — dash-fields-in-numeric-columns-read-as-null-and-typed-columns-aggregate [mismatch]:
  AssertionError: the engine typed the columns [['text', 'integer', 'real', 'integer', 'null']]
  expected [['date', 'bigint', 'double', 'integer', 'bigint']] from SELECT typeof("date"), typeof(sc_bytes), typeof(time_taken), typeof(sc_status), typeof(sc_range_start); the count/sum/max/min aggregate assertion over the same table passed.

Cause 4 — glue-accepts-a-location-in-an-absent-bucket-that-athena-rejects-at-query-time [mismatch]:
  AssertionError: the query over the absent bucket ended (state, category, type, reason) = ('FAILED', 2, None, 'Permission denied on S3 path')
  expected ('FAILED', 2, 1306, 'HIVE_FILESYSTEM_ERROR'), where 'reason' is StateChangeReason.split(':')[0]; the control table over the real bucket counted its 2 lines.

Cause 5 — glue-accepts-an-invalid-column-type-that-athena-rejects-at-query-time [mismatch]:
  AssertionError: the mistyped table's query ended (state, category, type, reason) = ('SUCCEEDED', None, None, '')
  expected ('FAILED', 2, 1100, 'HIVE_INVALID_METADATA') with 'sc_bytes' named in StateChangeReason; Glue had stored sc_bytes as 'notatype' and the correctly typed twin counted its 2 lines.

Cause 6 — athena-ddl-table-differs-from-the-glue-api-table-in-owner-partitions-and-location [mismatch]:
  AssertionError: the DDL route stored {'Owner': 'hadoop', 'PartitionKeys': None, 'NumberOfBuckets': -1, 'Location': 's3://<bucket>/logs', 'SerdeParameters': {'serialization.format': '\t', 'field.delim': '\t'}, 'SerdeLibrary': 'org.apache.hadoop.hive.serde2.lazy.LazySimpleSerDe', 'TableType': 'EXTERNAL_TABLE'}
  the only member diverging from the expected view is PartitionKeys: expected [], got None.

Smithy cited: com.amazonaws.athena#AthenaError (ErrorCategory integer range 1-3, ErrorType integer range 0-9999, ErrorMessage, Retryable default false), com.amazonaws.athena#ColumnInfo.Type (required), com.amazonaws.athena#QueryExecutionStatus.StateChangeReason, com.amazonaws.glue#Table.PartitionKeys. The HIVE_FILESYSTEM_ERROR/HIVE_INVALID_METADATA prefixes, the category/type values, the Glue denial message and the Hive-compatibility defaults are recorded behavior.

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Run simparity-behavioral-aws-v1 and inspect the six checks listed in trace_ids under the 'athena__glue__s3' heading.
3. Cause 1: create a Glue database/table over a seeded s3:// prefix, assume a role that has athena:* and s3:* but no glue:*, and run StartQueryExecution('SELECT * FROM <db>.<table>') as that role — expect State FAILED with the glue:GetDatabase denial message, observe SUCCEEDED and the row.
4. Cause 2: run a CREATE EXTERNAL TABLE and a SELECT count(*) through a workgroup with ResultConfiguration.OutputLocation s3://<bucket>/athena-results/, then ListObjectsV2 on that prefix — expect <ddl-id>.txt, <sel-id>.csv and <sel-id>.csv.metadata, observe an empty prefix.
5. Cause 3: over a Glue table declaring date/bigint/double/int/bigint columns, run SELECT typeof(<col>), … LIMIT 1 — expect the declared Athena type names, observe SQLite storage classes ('text'/'real'/'null').
6. Cause 4: register a table whose StorageDescriptor.Location points into a bucket that does not exist, query it, and read QueryExecution.Status — expect AthenaError.ErrorType 1306 and a 'HIVE_FILESYSTEM_ERROR: …' reason, observe no ErrorType and 'Permission denied on S3 path'.
7. Cause 5: register the same table with one column typed 'notatype' and run SELECT count(*) — expect FAILED with ErrorCategory 2 / ErrorType 1100 / 'HIVE_INVALID_METADATA: …' naming the column, observe SUCCEEDED.
8. Cause 6: create one table via glue.create_table and an equivalent one via Athena CREATE EXTERNAL TABLE, then glue.get_table on each — expect PartitionKeys [] on the DDL-created table (and absent on the API-created one), observe PartitionKeys absent on both.
