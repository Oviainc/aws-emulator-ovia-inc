Issue-Source: trace-sweep-divergence
Issue-Trace: schemas-schema-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: schemas-schema-lifecycle schemas-schema-search-and-list schemas-code-binding-lifecycle
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|CreateSchema|2xx->2xx|$.LastModified
Issue-Key: schemas|CreateSchema|2xx->2xx|$.VersionCreatedDate
Issue-Key: schemas|DescribeSchema|2xx->2xx|$.LastModified
Issue-Key: schemas|DescribeSchema|2xx->2xx|$.VersionCreatedDate
Issue-Key: schemas|UpdateSchema|2xx->2xx|$.LastModified
Issue-Key: schemas|UpdateSchema|2xx->2xx|$.VersionCreatedDate
Issue-Key: schemas|PutCodeBinding|2xx->2xx|$.CreationDate
Issue-Key: schemas|PutCodeBinding|2xx->2xx|$.LastModified
Issue-Key: schemas|DescribeCodeBinding|2xx->2xx|$.CreationDate
Issue-Key: schemas|DescribeCodeBinding|2xx->2xx|$.LastModified
Issue-Commit: e556c3079ace9bb74a6c235df6f1f92b13f370bd

# schemas renders date-time timestamps as epoch numbers in schema and code-binding responses

> _Note: this issue was identified against the code base at commit `e556c3079ace9bb74a6c235df6f1f92b13f370bd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchema

## Description
Every timestamp member in the schemas model targets `__timestampIso8601`, which carries `@timestampFormat("date-time")`. The emulator emits them as JSON numbers (epoch seconds with a fractional part), which is a different JSON type, not a different rendering: an SDK decoding a `date-time` member rejects a number. This bundle carries FIVE causes — the five operations whose recorded responses diverge this way — and all five must be closed.

Smithy (schemas model): `__timestampIso8601` = timestamp with timestampFormat date-time; CreateSchema/DescribeSchema/UpdateSchema outputs carry LastModified and VersionCreatedDate on that shape; DescribeCodeBinding (and PutCodeBinding) carry CreationDate and LastModified on it; SchemaSummary.LastModified likewise.

A known cross-service root cause is already open and names the generator that drops the trait: "`@timestampFormat` never reaches a generated member schema, so JSON timestamps go out as epoch seconds" (20261022T000000Z-smithy-macros-drops-timestampformat-from-member-schemas.md), and the near-duplicate 20261021T223000Z-scaffold-codegen-drops-timestampformat-from-member-schemas.md. Neither carries a schemas replay key, so this issue records the schemas fixtures that must pass once the format is honoured (or the affected shapes are written out by hand, as that issue describes).

Cause 1 of 5 — CreateSchema response (fixture schemas-schema-lifecycle, step task#2; cluster also covers schemas-schema-search-and-list#task#2 and #task#3).
  AWS: 201 with LastModified and VersionCreatedDate as RFC 3339 strings (recorded example '2026-09-06T05:39:37Z').
  Emulator: numbers, e.g. 1792687366.088722.
  Why: SDK deserialization of the create response fails or yields a wrong instant; clients cannot read back when a schema version was created.
  Fix: the CreateSchema response serializer / its output shape's member schemas (honour the modelled date-time format).

Cause 2 of 5 — DescribeSchema response (schemas-schema-lifecycle#task#3; cluster also covers task#5). Same AWS/Emulator divergence on LastModified and VersionCreatedDate. Fix: DescribeSchema response serializer.

Cause 3 of 5 — UpdateSchema response (schemas-schema-lifecycle#task#4). Same divergence on LastModified and VersionCreatedDate. Fix: UpdateSchema response serializer.

Cause 4 of 5 — PutCodeBinding response (schemas-code-binding-lifecycle#task#4).
  AWS: 202 with CreationDate and LastModified as date-time strings (the recording asserts the shape, not the instant).
  Emulator: numbers (1792687354.7666378).
  Fix: PutCodeBinding response serializer.

Cause 5 of 5 — DescribeCodeBinding response (schemas-code-binding-lifecycle#task#5). Same divergence on CreationDate and LastModified. Fix: DescribeCodeBinding response serializer.

The instant values themselves are environment-specific and are not the requirement; the JSON type and format are. Note the same defect also shows up inside ListSchemas/SearchSchemas item lists (SchemaSummary.LastModified, SearchSchemaVersionSummary.CreatedDate) in the clusters filed under the catalog and search issues.

Traces: schemas-schema-lifecycle#task#2 (representative), schemas-schema-lifecycle#task#3, schemas-schema-lifecycle#task#4, schemas-code-binding-lifecycle#task#4, schemas-code-binding-lifecycle#task#5.

## Evidence
Replay of simparity-wire-aws-v1:
- schemas-schema-lifecycle#task#2 CreateSchema 201->201: $.LastModified value_mismatch (expected '2026-09-06T05:39:37Z', got 1792687366.088722); $.VersionCreatedDate likewise
- schemas-schema-lifecycle#task#3 DescribeSchema 200->200: same two members, got 1792687366.088722
- schemas-schema-lifecycle#task#4 UpdateSchema 200->200: same two members, got 1792687366.0976605
- schemas-code-binding-lifecycle#task#4 PutCodeBinding 202->202: $.CreationDate / $.LastModified expected '<shaped-not-asserted>' (a date-time string), got 1792687354.7666378
- schemas-code-binding-lifecycle#task#5 DescribeCodeBinding 200->200: $.CreationDate got 1792687354.7666378, $.LastModified got 1792687355.1666377
Model evidence: smithy_query(query="shape", shape="__timestampIso8601") -> {"type": "timestamp", "timestampFormat": "date-time"}; smithy_query(query="operation", operation="CreateSchema,DescribeSchema,UpdateSchema,DescribeCodeBinding") shows LastModified/VersionCreatedDate/CreationDate targeting it with timestampFormat date-time.

## Steps to Reproduce
1. Replay fixture schemas-schema-lifecycle against the emulator.
2. Step task#2 (CreateSchema) returns LastModified/VersionCreatedDate as JSON numbers instead of RFC 3339 strings; steps task#3 (DescribeSchema) and task#4 (UpdateSchema) do the same.
3. Replay fixture schemas-code-binding-lifecycle: step task#4 (PutCodeBinding) and step task#5 (DescribeCodeBinding) return CreationDate/LastModified as numbers.
4. Any AWS SDK for this service fails to deserialize those members as date-time.
