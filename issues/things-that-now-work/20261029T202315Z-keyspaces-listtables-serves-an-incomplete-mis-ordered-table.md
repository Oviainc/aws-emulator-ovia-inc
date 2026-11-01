Issue-Source: trace-sweep-divergence
Issue-Trace: keyspaces-catalog-list-tables
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: keyspaces-catalog-list-tables
Issue-Shape-Id: com.amazonaws.keyspaces#KeyspacesService
Issue-Key: keyspaces|ListTables|2xx->2xx|$.tables[*]
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# keyspaces ListTables serves an incomplete, mis-ordered table list for the built-in system keyspaces

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTables

## Description
This issue bundles 3 net-new root causes in the keyspaces crate, one per built-in system keyspace. All three are the same KIND of defect — the emulator's seeded catalog for an AWS-managed keyspace holds the wrong set of tables, in the wrong order — but each keyspace's table list is a distinct piece of seed data, so each must be corrected separately. All three steps returned HTTP 200; only the tables list diverged. Three causes, all 3 must be closed before submitting.

Cause 1 of 3: ListTables(keyspaceName=system_schema_mcs) returns the wrong table at one position.
AWS: Smithy declares ListTables with required keyspaceName and output tables (TableSummaryList of TableSummary{keyspaceName, tableName, resourceArn}, all @required); the model does not enumerate the AWS-managed catalog's contents, so recorded behavior only. The recorded response for the system_schema_mcs keyspace is a fixed, service-owned list whose entries appear in a stable order; the entry the emulator serves at that position is not the one AWS serves there, and the table AWS serves (the keyspace's tables-history catalog table) is absent from the emulator's list while an entry AWS places elsewhere appears in its place. Each entry's resourceArn is arn:aws:cassandra:<region>:<accountId>:/keyspace/<keyspaceName>/table/<tableName>, which the emulator already builds correctly.
Emulator: serves a list of the right length for this keyspace but with a different composition/ordering, so a positional or set comparison against AWS fails.
Why: Cassandra drivers and schema tools read the AWS-managed schema catalogs to discover what metadata tables exist; a missing catalog table makes a feature that queries it look unsupported, and a different ordering breaks any client that pages or compares the list positionally.
Fix: the seed data for the system_schema_mcs keyspace's table set in the crate's keyspaces state (the fixed catalog the ListTables handler reads), and the order the handler returns entries in.
Traces: keyspaces-catalog-list-tables#task#2.

Cause 2 of 3: ListTables(keyspaceName=system) returns only the first few tables of the system keyspace.
AWS: recorded behavior only. The recorded response for the `system` keyspace is a long fixed list of the Cassandra system tables Amazon Keyspaces exposes (among them prepared_statements, size_estimates, IndexInfo, batches, paxos, peer_events, range_xfers, compaction_history, sstable_activity and further entries), in a stable service-defined order, each with the standard /keyspace/system/table/<tableName> ARN.
Emulator: serves only three entries and places a different table at index 1, so from index 3 onward the list is empty — more than a dozen of the recorded entries are absent.
Why: a driver that enumerates the system keyspace to decide which metadata it may query concludes those tables do not exist and silently disables the features that depend on them; nothing surfaces as an error, the catalog is just wrong.
Fix: the seed data for the `system` keyspace's table set in the crate's keyspaces state, and the handler's ordering.
Traces: keyspaces-catalog-list-tables#task#3.

Cause 3 of 3: ListTables(keyspaceName=system_multiregion_info) returns only one of its tables.
AWS: recorded behavior only. The recorded response for the system_multiregion_info keyspace carries the keyspace's multi-Region metadata tables — the keyspaces, types and autoscaling catalog tables in addition to the first entry the emulator already serves — each with the standard /keyspace/system_multiregion_info/table/<tableName> ARN.
Emulator: serves a single entry; the remaining recorded entries are absent.
Why: multi-Region-aware tooling that reads this keyspace to find per-Region replication and autoscaling metadata finds no such tables and treats multi-Region information as unavailable.
Fix: the seed data for the system_multiregion_info keyspace's table set in the crate's keyspaces state.
Traces: keyspaces-catalog-list-tables#task#4.

No open keyspaces issue covers the contents of the AWS-managed system keyspaces; the existing ListTables-adjacent issues concern user tables and auto scaling identity.

## Evidence
Replay of simparity-wire-aws-v1, all three steps returned the expected HTTP 200 — only $.tables diverged:

Cause 1 — keyspaces-catalog-list-tables#task#2, ListTables(system_schema_mcs):
  $.tables[5]: missing (expected {'keyspaceName': 'system_schema_mcs', 'tableName': 'tables_history', 'resourceArn': 'arn:aws:cassandra:us-west-2:000000000000:/keyspace/system_schema_mcs/table/tables_history'}, got the 'keyspaces' entry instead)

Cause 2 — keyspaces-catalog-list-tables#task#3, ListTables(system):
  $.tables[1]: expected 'prepared_statements', got 'peers'
  $.tables[3..]: expected 'size_estimates', 'IndexInfo', 'batches', 'paxos', 'peer_events', 'range_xfers', 'compaction_history', 'sstable_activity' and further entries; got None (list ends at index 2)

Cause 3 — keyspaces-catalog-list-tables#task#4, ListTables(system_multiregion_info):
  $.tables[1]: expected 'keyspaces', got None
  $.tables[2]: expected 'types', got None
  $.tables[3]: expected 'autoscaling', got None

The region and account id in the recorded ARNs are environment values the emulator already fills correctly; the requirement is the set of tableNames per system keyspace and the order they are returned in.

## Steps to Reproduce
Against the emulator's keyspaces endpoint, with no user keyspaces created:

Cause 1: ListTables(keyspaceName='system_schema_mcs') and compare the full tables list, in order, with the recorded one — the tables-history catalog entry must be present at its recorded position.
Cause 2: ListTables(keyspaceName='system') — expect the full Cassandra system-table catalog (prepared_statements at index 1, then size_estimates, IndexInfo, batches, paxos, peer_events, range_xfers, compaction_history, sstable_activity and the remaining recorded entries), not three entries.
Cause 3: ListTables(keyspaceName='system_multiregion_info') — expect the keyspaces, types and autoscaling entries in addition to the first one.

Each entry must carry keyspaceName, tableName and resourceArn=arn:aws:cassandra:<region>:<accountId>:/keyspace/<keyspaceName>/table/<tableName>. Replaying the fixture steps in Issue-Trace/Issue-Step and Issue-Trace-Ids reproduces each cause.
