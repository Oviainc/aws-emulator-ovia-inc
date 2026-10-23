Issue-Source: trace-sweep-divergence
Issue-Trace: keyspaces-catalog-list-keyspaces
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: keyspaces-catalog-list-keyspaces keyspaces-catalog-list-tables
Issue-Shape-Id: com.amazonaws.keyspaces#KeyspacesService
Issue-Key: keyspaces|ListKeyspaces|2xx->2xx|$.keyspaces[*]
Issue-Key: keyspaces|ListTables|2xx->4xx|$.tables
Issue-Key: keyspaces|ListTables|2xx->4xx|status
Issue-Commit: 7c3e3cd3d0ee17852e6b80a086a0aa798985a326

# Keyspaces catalog not seeded: system keyspaces and their system tables absent

> _Note: this issue was identified against the code base at commit `7c3e3cd3d0ee17852e6b80a086a0aa798985a326`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListKeyspaces

## Description
This issue bundles 2 causes, both rooted in the same defect: the emulator's Amazon Keyspaces state starts completely empty, with none of the service-managed (Cassandra) system keyspaces or their system tables present. On AWS these exist in every account/Region without the client ever creating them, so the whole read-only catalog surface of the service answers wrongly out of the box. BOTH causes must be closed before submitting.

Cause 1 of 2 - ListKeyspaces omits the service-managed keyspaces (fixture keyspaces-catalog-list-keyspaces, step task#1).
AWS: Smithy: ListKeyspaces -> ListKeyspacesResponse.keyspaces, target KeyspaceSummaryList, @required; KeyspaceSummary requires keyspaceName (@length 1..48, @pattern ^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$), resourceArn (@pattern ^arn:(aws[a-zA-Z0-9-]*):cassandra:.+) and replicationStrategy (SINGLE_REGION | MULTI_REGION). The model does not enumerate which keyspaces pre-exist, so for the catalog contents this is recorded behavior only: the recording shows ListKeyspaces on an untouched account returning HTTP 200 whose keyspaces list already contains the service-managed keyspaces system_schema, system_schema_mcs, system and system_multiregion_info, each with replicationStrategy SINGLE_REGION and a resourceArn of the form arn:aws:cassandra:<region>:<account>:/keyspace/<keyspaceName>/.
Emulator: answers HTTP 200 (status matches) but the response carries no entries at all - replay reports $.keyspaces[0] through $.keyspaces[3] as 'missing ... got None', i.e. the @required keyspaces member is empty or absent rather than listing the pre-existing keyspaces.
Why: a client enumerating keyspaces to discover the account's schema sees an empty catalog. Drivers and tooling that read Cassandra metadata out of system_schema/system first resolve the keyspace through ListKeyspaces, so they conclude the keyspace does not exist and either abort or try to create a keyspace whose name is reserved. Code that iterates keyspaces to locate the system catalog silently does nothing instead of failing loudly.
Fix: the Keyspaces state seed - the service's initial state must be constructed with the service-managed keyspaces present, and ListKeyspaces' handler must project them into KeyspaceSummary entries with keyspaceName, the ARN built from the request's region/account, and replicationStrategy.
Traces: keyspaces-catalog-list-keyspaces#task#1 (1 affected step).

Cause 2 of 2 - ListTables on a service-managed keyspace returns 404 instead of its tables (fixture keyspaces-catalog-list-tables, step task#1).
AWS: Smithy: ListTables input requires keyspaceName (@length 1..48, @pattern ^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$); output member tables targets TableSummaryList, and TableSummary requires keyspaceName, tableName and resourceArn (@pattern ^arn:(aws[a-zA-Z0-9-]*):cassandra:.+). ListTables models ResourceNotFoundException (client error, HTTP 404), documented as 'The operation tried to access a keyspace, table, or type that doesn't exist'. Because system_schema does exist on AWS, 404 is not the correct outcome for it. Which tables it holds is recorded behavior only: the recording shows HTTP 200 with the system_schema tables keyspaces, types, tables, columns, triggers, dropped_columns, views, functions, aggregates and indexes, each resourceArn of the form arn:aws:cassandra:<region>:<account>:/keyspace/<keyspaceName>/table/<tableName>.
Emulator: answers HTTP 404 where 200 is expected, and the body carries no tables member at all ($.tables 'missing ... got None'). The 404 is the ResourceNotFoundException path firing because the keyspace is not in state - the same missing seed as Cause 1, surfacing as an error rather than an empty list.
Why: a client gets a hard 404 ResourceNotFoundException for a keyspace that is always present on real AWS. Unlike an empty list this is a terminal error: SDK calls raise, retry logic treats a non-existent resource as unrecoverable, and any paginator over the system catalog fails on its first page. It also makes the two operations mutually inconsistent - nothing can be listed, and asking about what is missing errors instead of returning empty.
Fix: the same Keyspaces state seed, plus ListTables' handler - once the service-managed keyspaces carry their system tables, ListTables must resolve keyspaceName against state and project TableSummary entries; ResourceNotFoundException must remain reserved for keyspaces genuinely absent from state.
Traces: keyspaces-catalog-list-tables#task#1, and the cluster's other affected steps keyspaces-catalog-list-tables#task#2, #task#3, #task#4 (4 affected steps).

No open issue covers either cause: list_existing_issues returned no entries matching 'keyspaces'.

## Evidence
Replay of trace set simparity-wire-aws-v1: 2 fixtures, 5 steps, 0 passed, 5 mismatched (score 0.0%). emulator_up=true, bootstrap_signal=false, state_reset_failures=0, not_implemented=0 - so the emulator is serving and these are fidelity defects, not a bootstrap gap. No cluster carried a stub_path or an owned_by.

Cause 1 - divergence_key service=keyspaces|ListKeyspaces|status=200->200|keys=$.keyspaces[0],$.keyspaces[1],$.keyspaces[2],$.keyspaces[3]; verdict mismatch; expected_status 200, actual_status 200.
  $.keyspaces[0]: missing (expected {'keyspaceName': 'system_schema', 'replicationStrategy': 'SINGLE_REGION', 'resourceArn': 'arn:aws:cassandra:us-west-2:000000000000:/keyspace/system_schema/'}, got None)
  $.keyspaces[1]: missing (expected keyspaceName 'system_schema_mcs', replicationStrategy 'SINGLE_REGION', got None)
  $.keyspaces[2]: missing (expected keyspaceName 'system', replicationStrategy 'SINGLE_REGION', got None)
  $.keyspaces[3]: missing (expected keyspaceName 'system_multiregion_info', replicationStrategy 'SINGLE_REGION', got None)
  Requirement: HTTP 200 with the four service-managed keyspaces present as KeyspaceSummary entries. The region, account id and therefore the exact ARN text above are environment-specific examples, not requirements - the requirement is a well-formed arn:aws:cassandra:<region>:<account>:/keyspace/<name>/ matching the modeled @pattern.

Cause 2 - divergence_key service=keyspaces|ListTables|status=200->404|keys=$.tables,status; verdict mismatch; expected_status 200, actual_status 404.
  status: expected 200, got 404
  $.tables: missing (expected a 10-entry list of TableSummary for keyspace system_schema - tableName keyspaces, types, tables, columns, triggers, dropped_columns, views, functions, aggregates, indexes - got None)
  Requirement: HTTP 200 with the keyspace's tables, not the 404 / ResourceNotFoundException the emulator returns. The diverging status (200 vs 404) and the error class it implies are the requirements; the recorded ARN text and the account id 000000000000 are examples.

Both clusters are the same empty-state root cause: ListKeyspaces returns 200-with-nothing, ListTables turns the same absence into a 404.

## Steps to Reproduce
1. git_clone() the repo; the emulator is built from that checkout.
2. Confirm keyspaces is stood up: get_bootstrap_status() lists service-com-amazonaws-keyspaces-keyspacesservice under 'bootstrapped' (it is absent from 'pending'), and the replay summary reports emulator_up=true with bootstrap_signal=false.
3. replay_wire_corpus() against trace set simparity-wire-aws-v1 on a freshly reset emulator, so no fixture has created any keyspace.
4. Cause 1: replay fixture keyspaces-catalog-list-keyspaces, step task#1 - call ListKeyspaces with no keyspace ever created. Observe HTTP 200 whose keyspaces member carries no entries, where AWS returns the four service-managed keyspaces (system_schema, system_schema_mcs, system, system_multiregion_info), each with replicationStrategy SINGLE_REGION and a cassandra keyspace ARN.
5. Cause 2: replay fixture keyspaces-catalog-list-tables, step task#1 (and steps task#2, task#3, task#4 of the same fixture) - call ListTables with keyspaceName=system_schema. Observe HTTP 404 with no tables member, where AWS returns HTTP 200 and the 10 system_schema system tables as TableSummary entries.
6. After a fix, re-replay BOTH fixtures - keyspaces-catalog-list-keyspaces#task#1 and keyspaces-catalog-list-tables#task#1 - since the two causes sit in different operations and seeding state alone does not prove ListTables stopped raising ResourceNotFoundException.
