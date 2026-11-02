Issue-Source: trace-sweep-divergence
Issue-Trace: glue-cflog-get-tables-paginated-and-filtered
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-cflog-get-tables-paginated-and-filtered glue-gjwr-traj-catalog-is-regional-under-one-catalog-id glue-tsd-expression-filter-mistakes glue-tsd-next-token-reused-across-databases glue-gjwr-traj-region-sweep-locates-the-job glue-tsd-page-size-and-parameter-shape-mistakes
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetTables|2xx->2xx|$.NextToken
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*]
Issue-Key: glue|GetTables|4xx->2xx|$.Message
Issue-Key: glue|GetTables|4xx->2xx|$.__type
Issue-Key: glue|GetTables|4xx->2xx|status
Issue-Key: glue|ListJobs|4xx->2xx|$.Message
Issue-Key: glue|ListJobs|4xx->2xx|$.__type
Issue-Key: glue|ListJobs|4xx->2xx|status
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*].CatalogId
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*].DatabaseId
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*].DatabaseName
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*].IsRowFilteringEnabled
Issue-Key: glue|GetTables|4xx->2xx|$.message
Issue-Commit: c588c92daf57694e44caee6ddcc17c51be576a2d

# Glue pagination emits a raw-name NextToken, ignores page size and accepts invalid tokens

> _Note: this issue was identified against the code base at commit `c588c92daf57694e44caee6ddcc17c51be576a2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTables

## Description
Glue's paginated catalog and job listings are implemented with a plaintext cursor that is the last item's NAME, pages are not cut at the requested size, and a token the emulator did not mint (or an out-of-range MaxResults) is accepted instead of refused. Six causes sharing one root cause: the shared pagination helper behind GetTables/ListJobs neither opaquely encodes nor validates its cursor, and does not enforce the page boundary.

Cause 1 of 6: GetTables NextToken is the raw table name. AWS: Smithy GetTables is @paginated (inputToken/outputToken NextToken, pageSize MaxResults) and NextToken targets the opaque `Token` string shape; the recording asserts a shaped, opaque continuation token. Emulator: returns the literal name of the last table in the page ('cloudfront_access_logs' in the recording) as NextToken. Why: the token is guessable and forgeable, so a client can fabricate a cursor that real Glue would reject, and any client that logs or compares tokens sees catalog contents leaked into the cursor. Fix: the GetTables handler's pagination serializer must emit an opaque token, not catalog data. Traces: glue-cflog-get-tables-paginated-and-filtered#task#6.

Cause 2 of 6: GetTables returns more rows than the requested page and no NextToken. AWS: with MaxResults set, the page holds at most MaxResults tables and a NextToken is returned while more remain. Emulator: the response carries an extra TableList entry beyond the page and omits NextToken, so the caller believes the listing is complete. Why: a paging client silently sees a different page split than AWS and loses the continuation, which breaks any resumable enumeration. Fix: the GetTables handler's page-boundary logic. Traces: glue-cflog-get-tables-paginated-and-filtered#task#7.

Cause 3 of 6: GetTables accepts a NextToken it never issued. AWS: GetTables models InvalidInputException (httpError 400); the recording shows 400 InvalidInputException with `Message` 'Invalid token value' — entirely fixed text, no value echoed. Emulator: answers 200 and returns a page. Why: a corrupted or cross-request token silently restarts or skips the enumeration instead of failing loudly. Fix: GetTables' NextToken validation. Traces: glue-cflog-get-tables-paginated-and-filtered#task#10 (also glue-gjwr-traj-traj-catalog... cluster members glue-gjwr-traj-catalog-is-regional-under-one-catalog-id#task#4, glue-tsd-expression-filter-mistakes#task#8, glue-tsd-next-token-reused-across-databases#task#9).

Cause 4 of 6: ListJobs accepts an invalid pagination token. AWS: Smithy ListJobs is @paginated and models InvalidInputException (httpError 400); the recording shows 400 InvalidInputException with `Message` 'Invalid pagination token.' — entirely fixed text. Note Glue's wording here differs from GetTables' 'Invalid token value'; both are fixed per-operation strings. Emulator: answers 200. Fix: ListJobs' NextToken validation. Traces: glue-gjwr-traj-region-sweep-locates-the-job#task#8.

Cause 5 of 6: a GetTables NextToken minted for one database is honoured against another, and the cursor is again a raw table name. AWS: the recording asserts an opaque, shaped NextToken. Emulator: returns the raw name 'andes_3_0_data' as the cursor. Why: besides the opacity defect, a plaintext name-keyed cursor is inherently portable across databases, so a token reused against a different DatabaseName resolves instead of being rejected. Fix: the GetTables pagination cursor must be opaque AND bound to the request's DatabaseName/CatalogId. Traces: glue-tsd-next-token-reused-across-databases#task#8.

Cause 6 of 6: GetTables accepts MaxResults 1001. AWS: the recording shows 400 ValidationException with message "1 validation error detected: Value '1001' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 1000" — fixed template, with the rejected value and the member name taken from the request and the limit 1000 being Glue's rule for this member. (The checked-in Smithy model declares GetTables.MaxResults as CatalogGetterPageSize @range 1..100, so the live upper bound of 1000 is recorded behavior only; the emulator enforces neither.) Emulator: answers 200. Fix: GetTables' MaxResults range validation. Traces: glue-tsd-page-size-and-parameter-shape-mistakes#task#4.

Related open issue (does not cover these steps): 20261102T114851Z-glue-4-net-new-root-causes-createjob-and.md mentions a forged NextToken on GetTables.

## Evidence
Replay of trace set simparity-wire-aws-v1, service glue.

Cause 1 — glue-cflog-get-tables-paginated-and-filtered#task#6 GetTables, 200/200: $.NextToken value_mismatch (expected a shaped opaque token, got 'cloudfront_access_logs').
Cause 2 — glue-cflog-get-tables-paginated-and-filtered#task#7 GetTables, 200/200: $.NextToken missing (expected a shaped token, got None); $.TableList[1] extra (an entire extra table, 'elb_logs', beyond the requested page).
Cause 3 — glue-cflog-get-tables-paginated-and-filtered#task#10 GetTables: status expected 400, got 200; $.__type missing (expected 'InvalidInputException'); $.Message missing (expected 'Invalid token value'). 4 steps in cluster.
Cause 4 — glue-gjwr-traj-region-sweep-locates-the-job#task#8 ListJobs: status expected 400, got 200; $.__type missing (expected 'InvalidInputException'); $.Message missing (expected 'Invalid pagination token.').
Cause 5 — glue-tsd-next-token-reused-across-databases#task#8 GetTables, 200/200: $.NextToken value_mismatch (expected a shaped token, got 'andes_3_0_data').
Cause 6 — glue-tsd-page-size-and-parameter-shape-mistakes#task#4 GetTables: status expected 400, got 200; $.__type missing (expected 'ValidationException'); $.message missing (expected "1 validation error detected: Value '1001' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 1000").

## Steps to Reproduce
Replay the wire corpus and inspect these glue steps:
1. glue-cflog-get-tables-paginated-and-filtered#task#6 — GetTables with MaxResults; inspect NextToken.
2. glue-cflog-get-tables-paginated-and-filtered#task#7 — GetTables continuation; count TableList entries vs MaxResults.
3. glue-cflog-get-tables-paginated-and-filtered#task#10 — GetTables with a fabricated NextToken.
4. glue-gjwr-traj-region-sweep-locates-the-job#task#8 — ListJobs with an invalid NextToken.
5. glue-tsd-next-token-reused-across-databases#task#8 — GetTables token minted for a different database.
6. glue-tsd-page-size-and-parameter-shape-mistakes#task#4 — GetTables with MaxResults 1001.
