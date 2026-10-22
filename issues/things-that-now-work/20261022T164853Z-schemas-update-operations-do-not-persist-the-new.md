Issue-Source: trace-sweep-divergence
Issue-Trace: schemas-registry-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: schemas-registry-lifecycle schemas-schema-lifecycle
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|DescribeRegistry|2xx->2xx|$.Description
Issue-Key: schemas|DescribeSchema|2xx->2xx|$.Description
Issue-Related: 20261022T164853Z-schemas-renders-date-time-timestamps-as-epoch-numbers.md
Issue-Commit: e556c3079ace9bb74a6c235df6f1f92b13f370bd

# schemas update operations do not persist the new Description

> _Note: this issue was identified against the code base at commit `e556c3079ace9bb74a6c235df6f1f92b13f370bd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateRegistry

## Description
An update that changes only Description is accepted but not stored: a subsequent describe still reports the value from creation. This bundle carries TWO causes, both of which must be closed.

Cause 1 of 2 — UpdateRegistry's Description is not persisted (fixture schemas-registry-lifecycle, step task#6, DescribeRegistry).
  AWS: UpdateRegistry PUT /v1/registries/name/{RegistryName} with Description returns 200 echoing the new Description, and a following DescribeRegistry reports it (recorded: 'probe registry v2' after the update of a registry created with 'probe registry v1'). Smithy: UpdateRegistry input {RegistryName httpLabel, Description __stringMin0Max256}, output {Description, RegistryArn, RegistryName, tags}.
  Emulator: DescribeRegistry after the update still answers 200 with Description 'probe registry v1'.
  Why: a client cannot change a registry's description — the write silently no-ops, so read-after-write and any config-reconciliation loop diverge from AWS forever.
  Fix: the UpdateRegistry handler's state mutation (write Description back into the stored registry).

Cause 2 of 2 — UpdateSchema's Description is not persisted (fixture schemas-schema-lifecycle, step task#6, DescribeSchema).
  AWS: UpdateSchema PUT .../schemas/name/{SchemaName} with Description returns 200 and a following DescribeSchema reports the new value (recorded: 'v2' after a schema created with 'v1'). Smithy: UpdateSchema input {Description __stringMin0Max256, Content, Type, ClientTokenId}, output includes Description.
  Emulator: DescribeSchema still answers Description 'v1'. (The same step also shows the epoch-timestamp defect filed separately for the schema lifecycle responses; the Description regression is the cause here.)
  Fix: the UpdateSchema handler's state mutation (write Description — and in general the updated members — into the stored schema/version).

Traces: schemas-registry-lifecycle#task#6 (representative), schemas-schema-lifecycle#task#6.

## Evidence
Replay of simparity-wire-aws-v1 (0 state_reset_failures, so these are not cross-fixture state leaks):
- schemas-registry-lifecycle#task#6 DescribeRegistry 200->200: $.Description value_mismatch (expected 'probe registry v2', got 'probe registry v1') — the preceding UpdateRegistry step in the same fixture passed
- schemas-schema-lifecycle#task#6 DescribeSchema 200->200: $.Description value_mismatch (expected 'v2', got 'v1') (plus $.LastModified/$.VersionCreatedDate epoch numbers, filed under the timestamp issue)
Model evidence: smithy_query(query="operation", operation="UpdateRegistry,UpdateSchema") on the schemas model — both take Description and return it in their output.

## Steps to Reproduce
1. Replay fixture schemas-registry-lifecycle: CreateRegistry with Description 'probe registry v1', UpdateRegistry with Description 'probe registry v2', then DescribeRegistry (step task#6) — the emulator reports 'probe registry v1'.
2. Replay fixture schemas-schema-lifecycle: CreateSchema with Description 'v1', UpdateSchema with Description 'v2', then DescribeSchema (step task#6) — the emulator reports 'v1'.
3. Both update calls return 200, so the loss is only visible on read-back.
