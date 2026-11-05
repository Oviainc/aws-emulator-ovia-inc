Issue-Source: trace-sweep-divergence
Issue-Trace: glue-partition-index-and-partition-column-statistics
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-partition-index-and-partition-column-statistics glue-partition-index-lifecycle
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetPartitionIndexes|2xx->2xx|$.PartitionIndexDescriptorList[*]
Issue-Key: glue|GetPartitionIndexes|2xx->2xx|$
Issue-Key: glue|GetPartitionIndexes|2xx->2xx|$.PartitionIndexDescriptorList[*].IndexStatus
Issue-Commit: deff4a6e6d31627acea4018c09886a861433b132

# Glue partition indexes never become visible: GetPartitionIndexes always reports an empty list

> _Note: this issue was identified against the code base at commit `deff4a6e6d31627acea4018c09886a861433b132`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPartitionIndexes

## Description
Cause 1 of 1: a partition index registered on an existing table is never observable — GetPartitionIndexes answers 200 with an EMPTY PartitionIndexDescriptorList both immediately after the index is registered (where AWS reports it IndexStatus 'CREATING') and for the whole duration of a lifecycle poll (where AWS settles it to 'ACTIVE'). Two fixtures exercise the same missing state, so both replay keys are listed below and in trace_ids.

AWS: Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json) documents CreatePartitionIndex as 'Creates a specified partition index in an existing table' with required DatabaseName, TableName and PartitionIndex, and GetPartitionIndexes returns PartitionIndexDescriptorList whose PartitionIndexDescriptor has required IndexName, required Keys (KeySchemaElementList, @length min 1) and required IndexStatus (enum PartitionIndexStatus: CREATING, ACTIVE, DELETING, FAILED), documented 'CREATING: The index is being created'. Recorded behavior: once an index is registered on a table, GetPartitionIndexes(DatabaseName, TableName) lists exactly one descriptor for it — IndexName as submitted, Keys as submitted (name + type of each partition key), IndexStatus 'CREATING' right after registration and 'ACTIVE' once the build finishes, which is what the lifecycle fixture polls for.

Emulator: GetPartitionIndexes answers HTTP 200 with PartitionIndexDescriptorList == [] — the expected descriptor is simply absent, so IndexStatus is never reported at all and the poll exhausts its 60s/13-attempt budget. The read path itself is implemented and sound (services/service-com-amazonaws-glue-awsglue/src/ops/get_partition_indexes.rs resolves the catalog/database/table and then reads crate::state::partition_indexes::PartitionIndexStore, which models both CREATING and ACTIVE and a settle span), and CreateTable's PartitionIndexes member does write that store (services/service-com-amazonaws-glue-awsglue/src/ops/create_table.rs). What is missing is the post-create registration path: CreatePartitionIndex is still a scaffold that returns Scaffolded::not_implemented (HTTP 501 NotImplemented) and writes nothing (services/service-com-amazonaws-glue-awsglue/src/ops/create_partition_index.rs), so no record ever reaches the store these reads consult, and every GetPartitionIndexes downstream of it reports a table with no indexes.

Why: a client cannot create or observe a partition index on an existing table at all. Code that registers an index and then waits for CREATING -> ACTIVE (the documented lifecycle) never sees the index appear, so it either loops until its own timeout or concludes the index was silently dropped; query-planning and backfill logic keyed on an index being ACTIVE can never proceed, and a caller cannot distinguish 'index absent' from 'index still building'.

Fix: implement the CreatePartitionIndex handler (services/service-com-amazonaws-glue-awsglue/src/ops/create_partition_index.rs) so an accepted index is recorded in crate::state::partition_indexes::PartitionIndexStore keyed by the stored table — exactly as CreateTable's PartitionIndexes registration already does — and delete the SCAFFOLDED marker; the status progression CREATING -> ACTIVE then comes from the existing store and the existing GetPartitionIndexes read path needs no change.

Related open issues, named for the fixer — neither carries these wire fixture#step replay keys, so neither covers these steps (the replay reports both clusters as unowned): 20261025T232424Z-glue-createpartitionindex-stub.md asks for the operation to be implemented but is bound to behavioral check ids only, and 20261104T201414Z-glue-3-net-new-root-causes-createpartitionindex-key.md adds the IndexKeys-vs-PartitionColumns validation rule for the same handler from a behavioral check. Implementing the handler and its state write should close all three.

Traces: glue-partition-index-and-partition-column-statistics#task#7, glue-partition-index-lifecycle#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1, service glue. Both clusters are 200 expected / 200 observed with the response body missing the index.

Representative — glue-partition-index-and-partition-column-statistics#task#7, GetPartitionIndexes:
  $.PartitionIndexDescriptorList[0]: missing (expected {'IndexName': 'sim2real-probe-idx', 'IndexStatus': 'CREATING', 'Keys': [{'Name': 'dt', 'Type': 'string'}]}, got None)
  The index name and key name shown are the fixture's per-run values; the requirement is that the descriptor for the registered index is present with its submitted IndexName and Keys and with IndexStatus 'CREATING'.

Second binding — glue-partition-index-lifecycle#task#5, GetPartitionIndexes (polled):
  $: poll capped at 60s (expected {'PartitionIndexDescriptorList': [{'IndexName': 'yearidx', 'IndexStatus': 'ACTIVE', 'Keys': [{'Name': 'year', 'Type': 'string'}]}]}, got {'PartitionIndexDescriptorList': []})
  $.PartitionIndexDescriptorList[0]: missing
  $.PartitionIndexDescriptorList[0].IndexStatus: poll: not in ['ACTIVE'] after 13 attempt(s) (expected 'ACTIVE', got None)

Supporting code evidence: services/service-com-amazonaws-glue-awsglue/src/ops/create_partition_index.rs still carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker and its handle() body ends in Err(crate::scaffolded::Scaffolded::not_implemented("CreatePartitionIndex")) (line 168), while services/service-com-amazonaws-glue-awsglue/src/ops/get_partition_indexes.rs reads descriptors from the partition index store and services/service-com-amazonaws-glue-awsglue/src/state/partition_indexes.rs already models INDEX_STATUS_CREATING, INDEX_STATUS_ACTIVE and the settle span.

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1), then:
1. Inspect glue-partition-index-and-partition-column-statistics#task#7 — after the fixture creates the database, the table (with PartitionKeys) and registers a partition index on it, GetPartitionIndexes(DatabaseName=<db>, TableName=<table>) must return one descriptor with the submitted IndexName and Keys and IndexStatus 'CREATING'; observe PartitionIndexDescriptorList == [].
2. Inspect glue-partition-index-lifecycle#task#5 — the same read, polled: the descriptor must appear and its IndexStatus must reach 'ACTIVE'; observe an empty list for all 13 attempts until the 60s cap.
3. Minimal reproduction without the fixtures: CreateDatabase; CreateTable with PartitionKeys=[{Name:'dt',Type:'string'}]; CreatePartitionIndex(DatabaseName=<db>, TableName=<table>, PartitionIndex={IndexName:'idx', Keys:['dt']}) — observe HTTP 501 NotImplemented instead of 200; then GetPartitionIndexes -> observe an empty PartitionIndexDescriptorList where AWS lists 'idx' as CREATING and, shortly after, ACTIVE.
4. Control that must stay correct: a CreateTable that carries PartitionIndexes already registers indexes, so GetPartitionIndexes must keep reporting those.
5. Re-replay both fixture#step bindings (the representative and the one in trace_ids) to confirm the fix.
