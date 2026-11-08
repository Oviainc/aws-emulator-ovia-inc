Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__s3.glue-column-statistics-computed-from-real-data-match-the-known-distribution
Issue-Key: glue__s3.glue-column-statistics-computed-from-real-data-match-the-known-distribution|run#30|AssertionError|assert statistics["ColumnType"] == "long", (
Issue-Commit: 9e2a1177787692948f5e67ac213048cdaf07cba6

# glue__s3: computed column statistics echo the table's declared column type ('bigint') instead of the normalized statistics type ('long')

> _Note: this issue was identified against the code base at commit `9e2a1177787692948f5e67ac213048cdaf07cba6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetColumnStatisticsForTable

## Description
One root cause.

Cause 1 of 1: ColumnStatistics.ColumnType is reported as the declared Hive type rather than the statistics type AWS normalizes it to.
AWS: a Glue column-statistics task that scans the table's S3 data reports, for a column declared with Type 'bigint' in the table's StorageDescriptor.Columns, a ColumnStatistics entry whose StatisticsData.Type is LONG (the LongColumnStatisticsData variant) and whose ColumnType is 'long' — the integral types Glue collapses onto its LONG statistics domain are reported under that domain's own type name, not under the Hive spelling the table declared. Smithy: ColumnStatistics.ColumnType is a required TypeString documented as "The data type of the column", and ColumnStatisticsData.Type selects the LongColumnStatisticsData variant; the model does not state the bigint→long normalization, so that normalization is recorded behavior only. The statistics values themselves (min, max, null count, distinct count) are read back correctly in this check — only the reported type diverges.
Emulator: reports ColumnType 'bigint' — the string copied verbatim from the column definition the table was created with — while still populating the LongColumnStatisticsData variant, so the entry describes LONG statistics but names a type AWS never returns here.
Why: a client that keys off ColumnType to pick which StatisticsData variant to read, or that compares the reported type against the set of statistics types Glue can produce, sees a value outside that set and either mis-selects the variant or rejects the statistics as untyped; a catalog consumer that round-trips ColumnStatistics through UpdateColumnStatisticsForTable writes back a type AWS would not have produced.
Fix: the column-statistics task's result projection (the component that builds the ColumnStatistics entry from the scanned data) — it must set ColumnType to the normalized statistics type for the column's declared type rather than echoing the declared type, and the same normalization must apply to the GetColumnStatisticsForTable read path. The related open issue 20261025T170315Z-glue-2-net-new-root-causes-column-statistics.md (column-statistics task runs always settle FAILED and compute no statistics) describes an earlier state of this path and does not cover this check: statistics are now computed and the defect is in the type they are reported under.
Traces: glue__s3.glue-column-statistics-computed-from-real-data-match-the-known-distribution

## Evidence
glue__s3.glue-column-statistics-computed-from-real-data-match-the-known-distribution (mismatch)
AssertionError: statistics were computed as 'bigint', expected 'long' for a bigint column
(at the assertion `statistics["ColumnType"] == "long"`)

Setup: a CSV object under s3://<bucket>/stats/ with an `id` column (7 populated integral values, 2 empty rows) and a `tag` column; a Glue table whose StorageDescriptor.Columns declares id as Type 'bigint'; a column-statistics task run to terminal over column ['id'].
Expected (AWS): the ColumnStatistics entry for 'id' carries ColumnType 'long' with StatisticsData.Type LONG.
Actual (emulator): ColumnType 'bigint'.

## Steps to Reproduce
1. CreateBucket and PutObject a CSV at key 'stats/rows.csv' whose header is id,tag and whose rows give 'id' several integral values plus two rows with an empty id.
2. iam CreateRole trusting glue.amazonaws.com with read access to that bucket.
3. glue CreateDatabase, then glue CreateTable with TableType EXTERNAL_TABLE, Parameters {classification: csv, skip.header.line.count: '1'}, StorageDescriptor.Columns [{Name:'id',Type:'bigint'},{Name:'tag',Type:'string'}] and Location s3://<bucket>/stats/.
4. Start a column-statistics task for column 'id' with that role and poll it to a terminal state.
5. glue GetColumnStatisticsForTable for DatabaseName/TableName/ColumnName 'id' and read ColumnStatistics.ColumnType: AWS reports 'long', the emulator reports 'bigint'.
