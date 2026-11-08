Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__s3.athena-query-result-object-written-to-s3-output-location
Issue-Key: athena__s3.athena-query-result-object-written-to-s3-output-location|_poll_until#18|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 8f7c6ce558c7d939146a7c42cc95df5134e5d406

# Athena reports a query SUCCEEDED and names a result location, but writes no result object to S3

> _Note: this issue was identified against the code base at commit `8f7c6ce558c7d939146a7c42cc95df5134e5d406`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
One cause.

Cause 1 of 1: a query driven through an enforcing workgroup reaches state SUCCEEDED and GetQueryExecution reports ResultConfiguration.OutputLocation = <workgroup location><QueryExecutionId>.csv, but no object ever appears at that key in S3 — neither the result CSV nor its .metadata sibling.
Failing check ids: athena__s3.athena-query-result-object-written-to-s3-output-location

AWS: recorded behavior only (the Smithy model describes the control plane, not the result artifacts). Recorded: a SUCCEEDED query writes exactly two objects under the effective OutputLocation prefix — <QueryExecutionId>.csv containing a header row with the projected column names (here "marker") followed by the data rows, and <QueryExecutionId>.csv.metadata alongside it. The object key is formed from the effective OutputLocation prefix plus the QueryExecutionId; with EnforceWorkGroupConfiguration=true the workgroup's OutputLocation wins over a client-supplied ResultConfiguration.OutputLocation silently (no refusal), and nothing is written to the client-supplied bucket. Those names and ids come from the request and the minted execution id, not from any one recording.

Emulator: the control plane is consistent — the execution terminates SUCCEEDED and ResultConfiguration.OutputLocation reads back as the enforcing workgroup's prefix plus <QueryExecutionId>.csv — but the data plane writes nothing. HeadObject on that key 404s for the whole 120 s poll window ("Last result: False"), so the CSV and the .metadata sibling never exist.

Why: a client that does what the API tells it to do — run a query, read ResultConfiguration.OutputLocation, then GetObject that key (what the AWS CLI, the JDBC/ODBC drivers and most ETL glue code do rather than paging GetQueryResults) — gets NoSuchKey from a query the service just declared SUCCEEDED. Downstream steps that list the prefix see an empty result set and conclude the query returned no rows, which is indistinguishable from a genuinely empty result.

Fix: the Athena query execution path's result materialisation — the component that, on transition to SUCCEEDED, serialises the result set to <OutputLocation><QueryExecutionId>.csv and writes the .metadata sibling through S3. The output-location resolution itself is already correct and must stay so, including the EnforceWorkGroupConfiguration precedence.

Traces: athena__s3.athena-query-result-object-written-to-s3-output-location. Note the open issue 20261104T082656Z-athena-glue-s3-the-athena-data-plane-never.md records the same shape of defect for the athena__glue__s3 suite; it does not carry this check's replay key, so this is filed for the athena__s3 surface.

## Evidence
Check: athena__s3.athena-query-result-object-written-to-s3-output-location

Assertion from the replay:
  AssertionError: Athena writes the result object s3://sim2real-xsvc-athena-results-3efe0aa4dc79/results/39a34fca-b76c-4f94-895b-b0805bd7621c.csv not met within 120.0s. Last result: False
(the bucket name and query id are per-run values; the requirement is the key template <OutputLocation prefix><QueryExecutionId>.csv)

Expected (AWS):
  - GetQueryExecution: Status.State == SUCCEEDED, ResultConfiguration.OutputLocation == s3://<bucket>/results/<qid>.csv
  - HeadObject(<bucket>, results/<qid>.csv) succeeds
  - that CSV body contains the header cell "marker" (quoted) and the data row value
  - ListObjectsV2(<bucket>, Prefix=results/) also contains results/<qid>.csv.metadata
  - with EnforceWorkGroupConfiguration=true and a client-supplied ResultConfiguration.OutputLocation pointing at a decoy bucket: the query still SUCCEEDS, still reports the workgroup's location, still writes there, and the decoy bucket stays empty

Actual (emulator):
  - Status.State == SUCCEEDED and ResultConfiguration.OutputLocation is reported correctly
  - HeadObject on the reported key returns 404 for the entire 120 s window; no .csv and no .csv.metadata object is ever created

## Steps to Reproduce
1. CreateBucket <results-bucket> (and a second decoy bucket).
2. CreateWorkGroup with Configuration={ResultConfiguration:{OutputLocation: "s3://<results-bucket>/results/"}, EnforceWorkGroupConfiguration: true}.
3. StartQueryExecution(QueryString="SELECT 'sim2real-xsvc' AS marker", WorkGroup=<wg>); poll GetQueryExecution to a terminal state — it reports SUCCEEDED.
4. Read ResultConfiguration.OutputLocation from GetQueryExecution; it is s3://<results-bucket>/results/<qid>.csv.
5. HeadObject(Bucket=<results-bucket>, Key=results/<qid>.csv) — 404, indefinitely.
6. ListObjectsV2(Bucket=<results-bucket>, Prefix=results/) — empty; the expected results/<qid>.csv.metadata sibling is absent too.
