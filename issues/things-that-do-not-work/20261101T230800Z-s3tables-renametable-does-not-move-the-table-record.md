Issue-Source: trace-sweep-divergence
Issue-Trace: s3tables-rename-table-across-namespaces
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3tables-rename-table-across-namespaces s3tables-rename-table-same-namespace s3tables-rename-table-workflow s3tables-table-rename-and-tagging s3tables-table-metadata-location-update
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables|GetTable|2xx->4xx|$.createdAt
Issue-Key: s3tables|GetTable|2xx->4xx|$.createdBy
Issue-Key: s3tables|GetTable|2xx->4xx|$.format
Issue-Key: s3tables|GetTable|2xx->4xx|$.modifiedAt
Issue-Key: s3tables|GetTable|2xx->4xx|$.modifiedBy
Issue-Key: s3tables|GetTable|2xx->4xx|$.name
Issue-Key: s3tables|GetTable|2xx->4xx|$.namespace
Issue-Key: s3tables|GetTable|2xx->4xx|$.namespaceId
Issue-Key: s3tables|GetTable|2xx->4xx|$.ownerAccountId
Issue-Key: s3tables|GetTable|2xx->4xx|$.tableARN
Issue-Key: s3tables|GetTable|2xx->4xx|$.tableBucketId
Issue-Key: s3tables|GetTable|2xx->4xx|$.type
Issue-Key: s3tables|GetTable|2xx->4xx|$.versionToken
Issue-Key: s3tables|GetTable|2xx->4xx|$.warehouseLocation
Issue-Key: s3tables|GetTable|2xx->4xx|status
Issue-Key: s3tables|GetTable|4xx->2xx|$.message
Issue-Key: s3tables|GetTable|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: s3tables|GetTable|4xx->2xx|status
Issue-Key: s3tables|DeleteTable|2xx->4xx|status
Issue-Key: s3tables|DeleteNamespace|2xx->4xx|status
Issue-Key: s3tables|DeleteTableBucket|2xx->4xx|status
Issue-Related: 20261101T230800Z-s3tables-listtables-summaries-carry-epoch-timestamps-and-unrecorded.md
Issue-Commit: 8715ba3f180c27028dee3267eb478eb6947c4fd2

# s3tables RenameTable does not move the table record, leaving the old name live

> _Note: this issue was identified against the code base at commit `8715ba3f180c27028dee3267eb478eb6947c4fd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RenameTable

## Description
Six causes, all downstream of one defect: RenameTable answers as recorded but the stored table is never re-keyed, so the table stays under its old namespace/name and the new coordinates do not exist. Every cause must be closed before this issue is submitted.

Cause 1 of 6: GetTable at the new coordinates (fixture s3tables-rename-table-across-namespaces, step task#6).
AWS: Smithy RenameTable (PUT /tables/{tableBucketARN}/{namespace}/{name}/rename, 204, input newNamespaceName/newName/versionToken) moves the table; a following GetTable with the NEW namespace and name returns 200 with the full table — the recording shows name = the new name, namespace = [the new namespace], the SAME tableARN as before the rename (the ARN carries the server-minted table id, not the name), format ICEBERG, createdAt/modifiedAt, createdBy/modifiedBy/ownerAccountId, versionToken, warehouseLocation, type 'customer'.
Emulator: 404 NotFoundException — every member of the expected body is absent.
Why: a client that renames a table and then reads it (the documented way to confirm the rename) sees the table as deleted; the rename is silently lost even though the rename call itself reported success.
Fix: the RenameTable handler and the state it mutates — re-key the Table record under the new namespace/name (and update the tableARN -> TableLocation index) instead of leaving the old entry in place. Code: services/service-com-amazonaws-s3tables-s3tablebuckets/src/state.rs:278-302 (`TableLocation`, `tables_by_arn`) and the generated RenameTable op under src/ops/.
Traces: s3tables-rename-table-across-namespaces#task#6 (cluster also covers s3tables-rename-table-same-namespace#task#5, s3tables-rename-table-workflow#task#6 and #task#9, s3tables-table-rename-and-tagging#task#5).

Cause 2 of 6: GetTable at the OLD coordinates after the rename (same fixture, step task#7).
AWS: after a rename the old namespace/name no longer resolves — 404 NotFoundException with message "The specified table does not exist." (fixed text).
Emulator: 200 with the table body; no error at all.
Why: the old name stays addressable, so a caller that renamed to avoid a collision can still create/read the stale name and two paths appear to hold the same table.
Fix: as cause 1 — removing the old key is the same state mutation.
Traces: s3tables-rename-table-across-namespaces#task#7 (cluster also covers s3tables-rename-table-same-namespace#task#6).

Cause 3 of 6: ListTables for the source namespace (same fixture, step task#8).
AWS: after the table is renamed into another namespace, listing the SOURCE namespace returns no table summaries for it (recording: $.tables has no such element).
Emulator: 200 with the table still listed under the old namespace.
Why: inventory and cleanup logic that drains a namespace by listing it never converges.
Fix: as cause 1.
Traces: s3tables-rename-table-across-namespaces#task#8 (cluster also covers #task#9).

Cause 4 of 6: DeleteTable at the new coordinates during teardown (same fixture, step teardown#10).
AWS: 204 — the renamed table is deletable at its new namespace/name.
Emulator: 404.
Why: a renamed table cannot be deleted at the coordinates the client holds, so resources leak for the life of the process.
Fix: as cause 1.
Traces: s3tables-rename-table-across-namespaces#teardown#10 (cluster also covers s3tables-rename-table-same-namespace#teardown#8, s3tables-rename-table-workflow#teardown#10, s3tables-table-rename-and-tagging#teardown#11).

Cause 5 of 6: DeleteNamespace during teardown (same fixture, step teardown#11).
AWS: 204 — the namespace is empty once the table has been renamed away and deleted.
Emulator: 409 ConflictException, because the stale table is still keyed under that namespace.
Why: teardown of an empty namespace fails, so a test or client cannot return the account to a clean state.
Fix: as cause 1 (no change to DeleteNamespace's own emptiness rule).
Traces: s3tables-rename-table-across-namespaces#teardown#11 (cluster also covers s3tables-rename-table-same-namespace#teardown#9, s3tables-rename-table-workflow#teardown#11, s3tables-table-metadata-location-update#teardown#9, s3tables-table-rename-and-tagging#teardown#12).

Cause 6 of 6: DeleteTableBucket during teardown (same fixture, step teardown#13).
AWS: 204 — the bucket is empty after its namespaces are deleted.
Emulator: 409, because the namespace that still holds the stale table could not be deleted.
Why: the bucket can never be deleted, compounding the leak.
Fix: as cause 1.
Traces: s3tables-rename-table-across-namespaces#teardown#13 (cluster also covers s3tables-rename-table-same-namespace#teardown#10, s3tables-rename-table-workflow#teardown#13, s3tables-table-metadata-location-update#teardown#10, s3tables-table-rename-and-tagging#teardown#13).

## Evidence
Replay of simparity-wire-aws-v1 (6 clusters, 22 steps; the RenameTable call itself matched, so the divergence is entirely in the state it failed to change):
- GetTable task#6: status expected 200, got 404; $.name, $.namespace, $.tableARN, $.versionToken, $.format, $.createdAt/$.modifiedAt, $.createdBy/$.modifiedBy/$.ownerAccountId, $.warehouseLocation, $.type, $.namespaceId, $.tableBucketId all missing (got None).
- GetTable task#7 (old coordinates): status expected 404, got 200; expected $.message 'The specified table does not exist.', got None.
- ListTables task#8: $.tables[0] extra — the table is still listed under the source namespace.
- DeleteTable teardown#10: expected 204, got 404.
- DeleteNamespace teardown#11: expected 204, got 409.
- DeleteTableBucket teardown#13: expected 204, got 409.
The same pattern repeats in s3tables-rename-table-same-namespace, s3tables-rename-table-workflow, s3tables-table-rename-and-tagging and s3tables-table-metadata-location-update.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. Fixture s3tables-rename-table-across-namespaces: run through the RenameTable step, then step task#6 (GetTable at the new namespace/name) — it answers 404 instead of 200.
3. Step task#7: GetTable at the OLD namespace/name — it answers 200 instead of 404 'The specified table does not exist.'.
4. Step task#8: ListTables on the source namespace — the table is still listed.
5. Steps teardown#10/#11/#13: DeleteTable at the new coordinates (404 instead of 204), DeleteNamespace (409 instead of 204), DeleteTableBucket (409 instead of 204).
6. Repeat with s3tables-rename-table-same-namespace and s3tables-rename-table-workflow for the same-namespace rename path.
