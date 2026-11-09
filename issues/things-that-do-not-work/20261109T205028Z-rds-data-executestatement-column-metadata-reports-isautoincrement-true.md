Issue-Source: trace-sweep-divergence
Issue-Trace: rds-aurora-serverless-data-api-managed-secret-sql-round-trip
Issue-Step: task#12
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-aurora-serverless-data-api-managed-secret-sql-round-trip
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds-data|ExecuteStatement|2xx->2xx|$.columnMetadata[*].isAutoIncrement
Issue-Commit: fce22d92ce0f3b4c883797a9d5b97c4bc60b16e7

# rds-data: ExecuteStatement column metadata reports isAutoIncrement true for a non-identity column

> _Note: this issue was identified against the code base at commit `fce22d92ce0f3b4c883797a9d5b97c4bc60b16e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
One cause. The replay reported no owning issue and no blocking stub for it.

Cause 1 of 1: ExecuteStatement's columnMetadata marks a column as auto-incrementing when it is not.
AWS: Smithy `ColumnMetadata.isAutoIncrement` (target Boolean, `@default false`) is documented "A value that indicates whether the column increments automatically." The Data API derives it per result column from the backing table's column definition, so it is true only for a column the engine actually auto-increments (a MySQL AUTO_INCREMENT column, a PostgreSQL identity/serial column). For every other column — including computed/projected expressions and plain typed columns — AWS reports the modeled default, false.
Emulator: `ExecuteStatement` reports `isAutoIncrement: true` for a result column whose definition carries no auto-increment/identity property, i.e. the flag is set unconditionally (or from the column's ordinal position) rather than from the column definition.
Why: a client that reads `columnMetadata[].isAutoIncrement` to decide which columns to omit from an INSERT, to map a result set onto a generated-key column, or to build an ORM/schema projection concludes every column is server-generated. It then skips columns it must supply, or expects the engine to populate values it will not, and the write path breaks in ways that are invisible until the insert fails.
Fix: the rds-data ExecuteStatement result-metadata builder — populate `isAutoIncrement` from the resolved column's definition in the backing engine's catalog and otherwise emit the modeled default false, rather than hard-coding true.
Traces: rds-aurora-serverless-data-api-managed-secret-sql-round-trip#task#12.

## Evidence
Replay of trace set simparity-wire-aws-v1 (98 fixtures, 993 steps, score 49.8%).

rds-aurora-serverless-data-api-managed-secret-sql-round-trip#task#12, rds-data ExecuteStatement, status 200 -> 200:
  $.columnMetadata[0].isAutoIncrement: value_mismatch (expected False, got True)

The Smithy model gives `ColumnMetadata.isAutoIncrement` the default `false`; the recorded AWS response carries false for this column, and the emulator returns true.

## Steps to Reproduce
1. git_clone() then replay_wire_corpus() against the built emulator.
2. Replay fixture rds-aurora-serverless-data-api-managed-secret-sql-round-trip step task#12 — an rds-data ExecuteStatement against the Aurora Serverless cluster with includeResultMetadata requested.
3. Read columnMetadata[0].isAutoIncrement in the HTTP 200 response: the emulator returns true for a column with no auto-increment/identity definition, where AWS returns false.
