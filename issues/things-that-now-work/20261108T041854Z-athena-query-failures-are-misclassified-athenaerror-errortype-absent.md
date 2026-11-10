Issue-Source: trace-sweep-divergence
Issue-Trace: athena-cats-traj-table-location-mistakes
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: athena-cats-traj-table-location-mistakes athena-kfi-traj-athena-reads-the-iceberg-table
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecution.Status.AthenaError
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecution.Status.AthenaError.ErrorCategory
Issue-Related: 20261104T063259Z-athena-query-executions-never-succeed-statements-are-refused.md
Issue-Commit: 8c5329059e752b9fa65691305eadc8c02e100ead

# Athena query failures are misclassified: AthenaError ErrorType absent, ErrorCategory wrong, and an invalid table LOCATION succeeds (2 causes)

> _Note: this issue was identified against the code base at commit `8c5329059e752b9fa65691305eadc8c02e100ead`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueryExecution

## Description
TWO net-new causes in how the Athena query engine decides that a statement failed and how it describes the failure in QueryExecution.Status. The emulator either does not detect an AWS-fatal condition at all, or detects it and reports it with its own category/wording instead of Athena's standardized AthenaError.

Cause 1 of 2 — a CREATE EXTERNAL TABLE whose LOCATION points at an object instead of a prefix SUCCEEDS instead of failing (GetQueryExecution, fixture athena-cats-traj-table-location-mistakes step task#9).
AWS: Athena requires an external table's LOCATION to be an S3 prefix (a directory), not a single object key. A CREATE EXTERNAL TABLE whose LOCATION ends at an object settles State 'FAILED' with StateChangeReason and AthenaError.ErrorMessage both equal to the fixed text 'Cannot create table on file, please specify a directory' (no variable parts), AthenaError.ErrorCategory 2 (user error), AthenaError.ErrorType 1100 and Retryable false. Smithy com.amazonaws.athena#AthenaError models ErrorCategory (integer, range 1-3: 1 System, 2 User, 3 Other), ErrorType (integer, range 0-9999), Retryable (boolean, default false) and ErrorMessage; the category/type values themselves are recorded behavior.
Emulator: GetQueryExecution reports QueryExecution.Status.State 'SUCCEEDED' with no AthenaError and no StateChangeReason, and the replay's poll gave up after 3 attempts because 'SUCCEEDED' is in the fixture's fail_on set. The table is then created, which is why Glue's GetTables later reports the object key as the table Location (filed separately under glue).
Why: a client that validates its DDL against Athena by submitting it and checking the terminal state is told an invalid table definition is valid. The table it believes it created cannot be queried on AWS, so the divergence is silent at create time and only surfaces later, and any catalog state derived from it is wrong.
Fix: the StartQueryExecution/query-engine DDL path — validate that an external table LOCATION denotes a prefix and settle the execution FAILED with the AthenaError above when it does not.
Traces: athena-cats-traj-table-location-mistakes#task#9.

Cause 2 of 2 — an unreadable Iceberg table is reported with the emulator's own message, ErrorCategory 3 and no ErrorType (GetQueryExecution, fixture athena-kfi-traj-athena-reads-the-iceberg-table step task#7).
AWS: a SELECT over a Glue table marked as Iceberg but lacking a metadata location settles FAILED with AthenaError.ErrorCategory 2 (user error), AthenaError.ErrorType 1000, and StateChangeReason and AthenaError.ErrorMessage both equal to the fixed text 'GENERIC_USER_ERROR: Detected Iceberg type table without metadata location. Please make sure an Iceberg-enabled compute engine such as Athena or EMR Spark is used to create the table, or the table is created by using the Iceberg open source AWS library iceberg-aws. Setting table_type parameter in Glue metastore to create an Iceberg table is not supported.' That message has no variable parts — it names no database, table or SerDe.
Emulator: ErrorCategory 3 instead of 2, ErrorType absent entirely, and StateChangeReason/ErrorMessage replaced by the emulator-authored text 'the data of <database>.<table> is stored as <serdeLibrary>, which this emulator'"'"'s query engine does not read' — a message that both names the emulator and interpolates the database, table and SerDe names, none of which appear in AWS's message for this error class.
Why: ErrorCategory is the field a client branches on to decide whether a failure is the caller's fault (2) or the service's (3), and retry/alerting logic keyed to it will retry or page on a permanent user error. ErrorType is the documented code a client maps to a remediation; absent, there is nothing to map. The message leaks that the backend is an emulator and omits the GENERIC_USER_ERROR prefix clients and log scrapers match on.
Fix: the query engine's failure-classification and AthenaError construction — classify an Iceberg-table-without-metadata-location read as the modeled user error and emit AWS's fixed message, category 2 and type 1000 instead of the engine's own capability message. Note the open issue 20261104T163600Z-athena-statements-are-executed-by-a-sqlite-engine.md covers SQLite-derived diagnostics generally but does not carry this cluster's replay key.
Traces: athena-kfi-traj-athena-reads-the-iceberg-table#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 (36 fixtures, 384 steps, score 35.9%).

Cause 1 — athena-cats-traj-table-location-mistakes#task#9, GetQueryExecution, status 200 vs 200:
  $.QueryExecution.Status.State: value_mismatch (expected 'FAILED', got 'SUCCEEDED')
  $.QueryExecution.Status.AthenaError: missing (expected {'ErrorCategory': 2, 'ErrorMessage': 'Cannot create table on file, please specify a directory', 'ErrorType': 1100, 'Retryable': False}, got None)
  $.QueryExecution.Status.StateChangeReason: missing (expected 'Cannot create table on file, please specify a directory', got None)
  $.QueryExecution.Status.State: poll: 'SUCCEEDED' is in fail_on after 3 attempt(s)
  Statement submitted: CREATE EXTERNAL TABLE <db>.items (item_id string, name string, price int) ROW FORMAT DELIMITED FIELDS TERMINATED BY ',' LOCATION 's3://<bucket>/products/category_a/items.csv' TBLPROPERTIES ('skip.header.line.count'='1')

Cause 2 — athena-kfi-traj-athena-reads-the-iceberg-table#task#7, GetQueryExecution, status 200 vs 200:
  $.QueryExecution.Status.AthenaError.ErrorCategory: value_mismatch (expected 2, got 3)
  $.QueryExecution.Status.AthenaError.ErrorType: missing (expected 1000, got None)
  $.QueryExecution.Status.AthenaError.ErrorMessage: value_mismatch (expected 'GENERIC_USER_ERROR: Detected Iceberg type table without metadata location. Please make sure an Iceberg-enabled compute engine such as Athena or EMR Spark is used to create the table, or the table is created by using the Iceberg open source AWS library iceberg-aws. Setting table_type parameter in Glue metastore to create an Iceberg table is not supported.', got "the data of <db>.<table> is stored as org.apache.iceberg.mr.hive.HiveIcebergSerDe, which this emulator's query engine does not read")
  $.QueryExecution.Status.StateChangeReason: same value_mismatch as ErrorMessage
  Statement submitted: SELECT count(*) AS n FROM "<db>"."<table>"

Smithy cited: com.amazonaws.athena#AthenaError members ErrorCategory (integer, range 1-3 — 1 System, 2 User, 3 Other), ErrorType (integer, range 0-9999), Retryable (boolean, default false), ErrorMessage (string); GetQueryExecution output member QueryExecution. The specific category/type values and the message texts are recorded behavior.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1.
3. Cause 1: fixture athena-cats-traj-table-location-mistakes, step task#9 — submit a CREATE EXTERNAL TABLE through StartQueryExecution whose LOCATION is an S3 object key (…/items.csv) rather than a prefix, then poll GetQueryExecution. The emulator settles SUCCEEDED; AWS settles FAILED with AthenaError ErrorCategory 2 / ErrorType 1100 / ErrorMessage 'Cannot create table on file, please specify a directory'.
4. Cause 2: fixture athena-kfi-traj-athena-reads-the-iceberg-table, step task#7 — register a Glue table whose SerDe is HiveIcebergSerDe with no Iceberg metadata location, submit SELECT count(*) over it, and read QueryExecution.Status.AthenaError. Observe ErrorCategory 3, no ErrorType, and the emulator-authored message.
