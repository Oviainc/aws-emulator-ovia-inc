Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.rdapi-copy-from-s3-loads-object-rows-and-session-reports-count redshift.rdapi-copy-malformed-row-rejected-and-recorded-in-stl-load-errors redshift.redshift-copy-exceeding-maxerror-fails-and-leaves-the-table-empty redshift.redshift-copy-from-s3-loads-rows-and-records-the-one-bad-row-in-stl-load-errors redshift.redshift-copy-with-a-role-lacking-s3-read-fails-and-leaves-the-table-empty redshift.redshift-spectrum-external-table-returns-rows-from-s3-without-loading-them redshift.data-api-execute-statement-typed-result-roundtrip
Issue-Key: redshift|PutObject|NotImplemented
Issue-Key: redshift.data-api-execute-statement-typed-result-roundtrip|run#31|AssertionError|assert columns[0]["typeName"] == "int4", (
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Redshift COPY/Spectrum behavioral checks blocked by the S3 tagged-PutObject stub, and the Data API types an integer-literal column int8 instead of int4 (2 causes)

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
Replay of simparity-behavioral-aws-v1 against the redshift emulator left two net-new root causes after the suite's own ALREADY FILED / OWNED clusters were removed (DescribeOrderableClusterOptions, CreateClusterParameterGroup, DescribeClusterVersions, BatchExecuteStatement and the sagemaker-owned GetClusterCredentials check are all skipped here). This bundle carries TWO causes; both must be closed before submitting.

=== Cause 1 of 2: S3 PutObject refuses every request that carries the x-amz-tagging header, so no Redshift COPY/Spectrum check can stage its source object ===
AWS: Smithy (com-amazonaws-s3-amazons3): PutObject's input member `Tagging` is bound to @httpHeader("x-amz-tagging"); PutObject is `PUT /{Bucket}/{Key+}` with success status 200 and its modeled errors are only EncryptionTypeMismatch, InvalidRequest, InvalidWriteOffset and TooManyParts. A PutObject that supplies object tags therefore stores the object and answers 200 with an ETag exactly as an untagged PutObject does; `NotImplemented` is not a modeled outcome of this operation at all.
Emulator: the call fails with error code `NotImplemented`; recorded message example: "operation PutObject with x-amz-tagging is scaffolded but not implemented in this emulator". Untagged PutObject works, so this is a partially-implemented operation rather than a plain scaffold stub — which is why the suite did not mark these six checks as blocked-and-already-filed.
Why: every Redshift COPY / Spectrum scenario stages its CSV in S3 with a `sim2real` tag before running COPY or querying an external table. The upload fails with a non-modeled error, so six behavioral checks never reach a single Redshift assertion: COPY row loading, STL_LOAD_ERRORS recording, MAXERROR behaviour, the role-without-s3:GetObject failure path and Spectrum external-table reads are all unverifiable. A client that uploads an object together with its tags (the normal way to stage a tagged data file) gets a failure instead of a stored object.
Fix: the s3 PutObject handler/validation (services/service-com-amazonaws-s3-amazons3/src/ops/put_object.rs) — parse the Tagging header into the object's tag set and store it instead of refusing the request. NOTE: the identical root cause is already described as Cause 2 of the open issue 20261022T135641Z-bedrock-agentcore-control-is-unreachable-router-keys-on.md (grounded in bedrock-agentcore-control.agent-runtime-endpoint-pinned-version-survives-runtime-update). It is repeated here only because that issue carries none of the redshift replay keys below, so a fixer closing it would never re-replay these six checks; close both together.
Traces: redshift.rdapi-copy-from-s3-loads-object-rows-and-session-reports-count, redshift.rdapi-copy-malformed-row-rejected-and-recorded-in-stl-load-errors, redshift.redshift-copy-exceeding-maxerror-fails-and-leaves-the-table-empty, redshift.redshift-copy-from-s3-loads-rows-and-records-the-one-bad-row-in-stl-load-errors, redshift.redshift-copy-with-a-role-lacking-s3-read-fails-and-leaves-the-table-empty, redshift.redshift-spectrum-external-table-returns-rows-from-s3-without-loading-them.

=== Cause 2 of 2: Data API ColumnMetadata.typeName reports int8 for a column that Redshift types int4 ===
AWS: Smithy (com-amazonaws-redshiftdata-redshiftdata): ColumnMetadata.typeName is documented as "The database-specific data type of the column", so the value is whatever the Redshift engine's catalogue reports and the model does not enumerate it — recorded behavior: a SELECT whose column is a bare integer literal is typed `int4` by Redshift (precision 10, isSigned true, length 0), and the check also asserts varchar columns carry their declared width in `precision`.
Emulator: the same column comes back with typeName `'int8'`, so the engine's type for an integer literal is a 64-bit integer where Redshift reports a 32-bit one. The check fails on the first column before the remaining typeName/precision assertions are reached.
Why: typeName is the only place a Data API client learns the column's server-side type; drivers and ETL tools map int4 -> 32-bit integer and int8 -> 64-bit, and a client that round-trips DDL or generates target schemas from ColumnMetadata produces the wrong column type for every integer expression.
Fix: the redshift-data result-metadata/type-mapping component that builds ColumnMetadata from the query engine's column descriptors — map an integer expression's type to Redshift's int4 (and keep the per-type precision/isSigned/isCaseSensitive flags the check asserts) rather than defaulting every integer to int8.
Traces: redshift.data-api-execute-statement-typed-result-roundtrip.

## Evidence
Suite: simparity-behavioral-aws-v1, redshift 0/29 (0%), 29 failed; this issue covers the 7 failures that are neither ALREADY FILED nor OWNED.

Cause 1 (6 checks, verdict=error, identical detail):
  expected: PutObject with Tagging/x-amz-tagging -> HTTP 200 + ETag, object stored with its tag set
  actual:   botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutObject operation: operation PutObject with x-amz-tagging is scaffolded but not implemented in this emulator
  affected: redshift.rdapi-copy-from-s3-loads-object-rows-and-session-reports-count, redshift.rdapi-copy-malformed-row-rejected-and-recorded-in-stl-load-errors, redshift.redshift-copy-exceeding-maxerror-fails-and-leaves-the-table-empty, redshift.redshift-copy-from-s3-loads-rows-and-records-the-one-bad-row-in-stl-load-errors, redshift.redshift-copy-with-a-role-lacking-s3-read-fails-and-leaves-the-table-empty, redshift.redshift-spectrum-external-table-returns-rows-from-s3-without-loading-them

Cause 2 (1 check, verdict=mismatch):
  check_redshift_data_api_execute_statement_typed_result_roundtrip.py line 289
  assert columns[0]["typeName"] == "int4"
  AssertionError: integer literal column must be typed int4, got 'int8'
  (same file goes on to assert columns[1]/columns[3] typeName 'varchar', columns[1] precision 32, columns[0] isSigned True — none of those were reached)

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; point botocore at it.
2. Cause 1: s3.put_object(Bucket=b, Key=k, Body=b"a,b\n1,2\n", Tagging="sim2real=true") -> ClientError NotImplemented; the identical call without Tagging succeeds. Then re-replay redshift.redshift-copy-from-s3-loads-rows-and-records-the-one-bad-row-in-stl-load-errors (and the five other COPY/Spectrum checks in trace_ids), each of which stages its CSV with a tag before running COPY.
3. Cause 2: run a Data API statement whose select list starts with a bare integer literal (e.g. "select 1 as n, cast('x' as varchar(32)) as s") against a provisioned cluster, poll DescribeStatement to FINISHED, then GetStatementResult and read ColumnMetadata[0]["typeName"]. Expect 'int4'; observe 'int8'.
4. Re-replay every check id in trace_ids to confirm the fix.
