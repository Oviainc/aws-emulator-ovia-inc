Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ba63228bbc1032132c1fbdbb0b54a8c844ce19f7

# CreateTable accepts staticColumns on a table whose schema declares no clusteringKeys

_Filed: 2026-10-24T03:46:38Z_
_Operation: CreateTable | Class: validation_

> _Note: this issue was identified against the code base at commit `ba63228bbc1032132c1fbdbb0b54a8c844ce19f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Amazon Keyspaces is CQL/Cassandra compatible, and in CQL a static column is only meaningful — and only legal — when the table has at least one clustering column: a static value is shared by all rows of a partition, so a table with no clustering columns (one row per partition) has nothing to share. Cassandra refuses such a definition ("Static columns are only useful (and thus allowed) if the table has at least one clustering column"), and Keyspaces surfaces that as the ValidationException CreateTable declares: "The operation failed due to an invalid or malformed request."

The emulator's schema validation checks that every name in partitionKeys/clusteringKeys/staticColumns is declared in allColumns, that no column is declared twice, and that no column takes two roles — but it never checks the staticColumns/clusteringKeys relationship. A schema with partitionKeys + staticColumns and no clusteringKeys is accepted with 200, stored, and reported back by GetTable, where the real service refuses the create and stores nothing.

## Evidence

Spec/well-established behavior: `SchemaDefinition$staticColumns` documents static columns as the columns "shared across all rows of a partition" of the table; the CQL rule that they require at least one clustering column is the storage model Keyspaces implements, and Keyspaces reports invalid DDL through CreateTable's ValidationException (HTTP 400).

Handler: services/service-com-amazonaws-keyspaces-keyspacesservice/src/state.rs `validate_schema` (around lines 1317-1359) enforces only declaration, uniqueness and single-role; `create_table.rs` calls it and then stores the schema as supplied. No rule anywhere relates staticColumns to clusteringKeys.

Tests: tests/all/keyspace_table_lifecycle.rs (`requests_that_break_an_unmodelled_rule_are_refused_and_the_rest_accepted`) and tests/all/column_types_and_auto_scaling.rs cover the duplicate-name/undeclared-name/role-conflict refusals, but none covers a static column on a schema without clustering keys, so no test asserts the current acceptance either.

## Steps to Reproduce

1. CreateKeyspace {keyspaceName: ks}
2. CreateTable {keyspaceName: ks, tableName: t, schemaDefinition: {allColumns: [{name: pk, type: int}, {name: s, type: text}], partitionKeys: [{name: pk}], staticColumns: [{name: s}]}} — no clusteringKeys
3. Expected: 400 ValidationException and no table stored; actual: 200 with a resourceArn
4. GetTable {keyspaceName: ks, tableName: t} — the emulator reports the accepted schema including staticColumns, where AWS has no such table
