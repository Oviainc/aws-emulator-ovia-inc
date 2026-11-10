Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__s3.athena-query-result-object-written-to-s3-output-location
Issue-Key: athena__s3.athena-query-result-object-written-to-s3-output-location|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Commit: 76dff93be94f91562bba6a25a3aecfc1e2578190

# athena__s3: a SELECT never produces its result CSV and .metadata sibling at the workgroup/request OutputLocation

> _Note: this issue was identified against the code base at commit `76dff93be94f91562bba6a25a3aecfc1e2578190`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution / GetQueryExecution (result object in S3)

## Description
One net-new root cause in the athena__s3 suite. It is NOT covered by the open issue 20261104T083348Z-athena-s3-athena-refuses-create-database-and-aliased.md, which carries the other athena__s3 checks' replay keys for a CREATE DATABASE refusal; this check carries none.

Cause 1 of 1: a query's result artifacts never appear at the output location, so neither the result-object layout nor the enforcing-workgroup override can be observed.
AWS: recorded behavior only for the object layout; the surface is modelled. StartQueryExecution for a SELECT writes the result set to <OutputLocation><QueryExecutionId>.csv and a sibling <QueryExecutionId>.csv.metadata at the same prefix, and GetQueryExecution reports State SUCCEEDED with ResultConfiguration.OutputLocation naming that object. When the workgroup's ResultConfigurationUpdates set EnforceWorkGroupConfiguration, a client-supplied OutputLocation is overridden silently — the objects land under the workgroup's location and nothing is written to the client's — and the override is not reported as an error.
Emulator: the check is reported "Error: Check timed out after 60.0s" with no assertion reached. The wait for the query to reach a terminal state and for its result objects to be listable never completes, so neither the .csv/.metadata pair, nor the decoy-bucket emptiness that proves the enforcing override, was ever compared.
Why: a client cannot retrieve any Athena result. Every consumer that reads <QueryExecutionId>.csv from the output location — the console, the CLI's athena get-query-results path, every ETL job that picks up the result object — gets nothing, and an administrator cannot verify that EnforceWorkGroupConfiguration actually confines results to the governed bucket.
Fix: the Athena query-execution engine's result-writing step — a terminal SUCCEEDED query must write <QueryExecutionId>.csv plus <QueryExecutionId>.csv.metadata under the effective output location, where the effective location is the workgroup's whenever EnforceWorkGroupConfiguration is set, and GetQueryExecution must report that location.
Traces: athena__s3.athena-query-result-object-written-to-s3-output-location.

## Evidence
Check athena__s3.athena-query-result-object-written-to-s3-output-location, verdict [mismatch]:
  Error: Check timed out after 60.0s
with no assertion reached.

The awaited requirement, from the check's own docstring: "Athena writes the result CSV with the query's rows to <OutputLocation><query_id>.csv plus a .metadata sibling, and an enforcing workgroup's location overrides a client-supplied one silently." The check creates two buckets — a results bucket (prefix "results/", output_location s3://<bucket>/results/) and a decoy bucket that must stay empty under the enforcing workgroup.

Expected: after the query reaches SUCCEEDED, ListObjectsV2 under results/ shows <QueryExecutionId>.csv with the query's rows and <QueryExecutionId>.csv.metadata, and the decoy bucket holds nothing. Actual: the query never reaches a terminal state and no object is ever listed, so the check is killed with nothing compared. Bucket names and query execution ids are per-run values.

## Steps to Reproduce
1. Create a results bucket and a decoy bucket.
2. StartQueryExecution for a SELECT with ResultConfiguration.OutputLocation = s3://<results bucket>/results/ and poll GetQueryExecution for SUCCEEDED; ListObjectsV2 under results/ and look for <QueryExecutionId>.csv and <QueryExecutionId>.csv.metadata.
3. CreateWorkGroup (or UpdateWorkGroup) with ResultConfiguration.OutputLocation = s3://<results bucket>/results/ and EnforceWorkGroupConfiguration=true; StartQueryExecution in that workgroup supplying the DECOY bucket as OutputLocation and confirm the objects land under the workgroup's location while the decoy stays empty, with no error.
4. The emulator never reaches a terminal query state. Re-replay athena__s3.athena-query-result-object-written-to-s3-output-location to confirm the fix.
