Issue-Source: trace-sweep-divergence
Issue-Trace: glue-tsd-attributes-to-get-names-then-describe
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-tsd-attributes-to-get-names-then-describe glue-tsd-expression-filter-mistakes glue-tsd-next-token-reused-across-databases glue-tsd-foreign-and-malformed-catalog-id glue-tsd-page-size-and-parameter-shape-mistakes glue-tsd-search-tables-route
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*].IsMaterializedView
Issue-Related: 20261102T132146Z-glue-pagination-emits-a-raw-name-nexttoken-ignores.md
Issue-Commit: c588c92daf57694e44caee6ddcc17c51be576a2d

# Glue GetTables omits catalog and database identity members from returned tables

> _Note: this issue was identified against the code base at commit `c588c92daf57694e44caee6ddcc17c51be576a2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTables

## Description
When GetTables is called with AttributesToGet (a projected listing), every Table the emulator returns is stripped of the identity and row-filtering members that real Glue always writes onto a table, even in a projected response. Three causes, all the same serializer gap on a different request shape.

Cause 1 of 3: GetTables with AttributesToGet returns tables lacking CatalogId, DatabaseId, DatabaseName and IsRowFilteringEnabled. AWS: Smithy Table declares CatalogId (CatalogIdString), DatabaseName (NameString), DatabaseId and IsRowFilteringEnabled — the latter two documented as "Recorded on the wire but absent from the published Smithy model" — and the recording shows every returned table carrying the catalog id, the owning database's name and id, and IsRowFilteringEnabled=false. Emulator: those four members are absent from each TableList entry. Why: a client cannot tell which catalog or database a listed table belongs to, so any code that keys a listing by DatabaseName (or checks whether row-level security is in force) reads null. Fix: the GetTables output serializer must populate Table's identity members regardless of the AttributesToGet projection. Traces: glue-tsd-attributes-to-get-names-then-describe#task#7 (cluster also covers glue-tsd-expression-filter-mistakes#task#10, glue-tsd-next-token-reused-across-databases#task#10).

Cause 2 of 3: the same projected GetTables call additionally drops IsMaterializedView. AWS: Smithy Table.IsMaterializedView (NullableBoolean) is written on the wire; the recording shows IsMaterializedView=false alongside the four members from cause 1. Emulator: CatalogId, DatabaseId, DatabaseName, IsMaterializedView and IsRowFilteringEnabled are all absent. Fix: same serializer; the default-valued booleans must be emitted, not elided. Traces: glue-tsd-attributes-to-get-names-then-describe#task#8 (cluster also covers glue-tsd-foreign-and-malformed-catalog-id#task#8, glue-tsd-page-size-and-parameter-shape-mistakes#task#9).

Cause 3 of 3: the same omission on a multi-row projected GetTables page — EVERY entry in the list, not just the first, is missing CatalogId, DatabaseId, DatabaseName and IsRowFilteringEnabled. AWS: as above, per table. Emulator: TableList[0] and TableList[1] both lack all four. Why: this confirms the gap is in the per-table serializer rather than a first-element accident, so the whole projected page is unusable for identity. Fix: same serializer. Traces: glue-tsd-expression-filter-mistakes#task#9 (cluster also covers glue-tsd-expression-filter-mistakes#task#11, glue-tsd-search-tables-route#task#13).

Related open issue (does not cover these steps): 20261029T201859Z-glue-metadata-catalog-reads-omit-modeled-members-and.md.

## Evidence
Replay of trace set simparity-wire-aws-v1, service glue. All three clusters are 200/200 body mismatches.

Cause 1 — glue-tsd-attributes-to-get-names-then-describe#task#7 GetTables: $.TableList[0].CatalogId missing (expected '000000000000'), $.TableList[0].DatabaseId missing (expected a shaped id), $.TableList[0].DatabaseName missing (expected the owning database name), $.TableList[0].IsRowFilteringEnabled missing (expected False). 3 steps in cluster.
Cause 2 — glue-tsd-attributes-to-get-names-then-describe#task#8 GetTables: the same four plus $.TableList[0].IsMaterializedView missing (expected False). 3 steps in cluster.
Cause 3 — glue-tsd-expression-filter-mistakes#task#9 GetTables: CatalogId, DatabaseId, DatabaseName and IsRowFilteringEnabled missing on BOTH $.TableList[0] and $.TableList[1]. 3 steps in cluster.

## Steps to Reproduce
Replay the wire corpus and inspect these glue steps:
1. glue-tsd-attributes-to-get-names-then-describe#task#7 — GetTables with AttributesToGet; inspect TableList[0].
2. glue-tsd-attributes-to-get-names-then-describe#task#8 — same, also checking IsMaterializedView.
3. glue-tsd-expression-filter-mistakes#task#9 — GetTables with Expression returning two tables; inspect both entries.
In each the returned Table objects lack CatalogId / DatabaseId / DatabaseName / IsRowFilteringEnabled.
