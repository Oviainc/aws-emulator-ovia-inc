Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4ffec8ed9c134267b14eefb2b325da9a0803e3ce

# GetTable never reports replicaSpecifications, even for a table in a MULTI_REGION keyspace

_Filed: 2026-10-24T00:49:58Z_
_Operation: GetTable | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4ffec8ed9c134267b14eefb2b325da9a0803e3ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

GetTable hardcodes `replica_specifications: None` (together with latestStreamArn / cdcSpecification / warmThroughputSpecification) in every response, regardless of the keyspace's replication strategy.

Expected (AWS): `GetTableResponse$replicaSpecifications` "Returns the Amazon Web Services Region specific settings of all Regions a multi-Region table is replicated in". A table created in a keyspace whose `replicationStrategy` is `MULTI_REGION` (which this emulator fully supports — CreateKeyspace stores the region list and GetKeyspace reports `replicationRegions` and `replicationGroupStatuses`) is a multi-Region table, so GetTable must return one `ReplicaSpecificationSummary` per replication Region, each carrying `region`, `status` and `capacitySpecification` (CapacitySpecificationSummary). Only a single-Region table legitimately reports none.

Actual (emulator): the member is a literal `None` for every table, so a caller cannot distinguish a single-Region table from a table in a 3-Region keyspace, and the per-Region status/capacity a multi-Region client reads before switching Regions is never there. The identical hardcode exists in `GetTableAutoScalingSettings` (`replica_specifications: None`), whose output member is documented as "the settings for all Regions the table is replicated in" — same root cause, same fix site (derive the per-Region summaries from the owning keyspace's `replication_regions` plus the table's stored capacity/auto-scaling settings).

Note: the absence of `cdcSpecification` / `latestStreamArn` is already tracked by the open "Keyspaces CDC never enabled" issue and is not what this issue asks for.

## Evidence

Spec: Smithy `GetTableResponse$replicaSpecifications` -> list<ReplicaSpecificationSummary> (len 0..), doc "Returns the Amazon Web Services Region specific settings of all Regions a multi-Region table is replicated in"; `ReplicaSpecificationSummary` members region/status/capacitySpecification; `GetTableAutoScalingSettingsResponse$replicaSpecifications` -> list<ReplicaAutoScalingSpecification>.
Handler: services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/get_table.rs lines 260-267 (`replica_specifications: None` with a comment that nothing can configure it) and src/ops/get_table_auto_scaling_settings.rs lines 236-239. The state record `crate::state::Table` (src/state.rs lines 594-645) has no replica field at all, while the owning `Keyspace` DOES store `replication_strategy` and `replication_regions` (src/state.rs lines 542-570), so the information needed to derive the summaries is already in state.
Tests: tests/all/keyspace_table_lifecycle.rs and tests/all/get_keyspace.rs exercise MULTI_REGION keyspaces, but no test asserts GetTable's replicaSpecifications for a table inside one; nothing covers this.

## Steps to Reproduce

1. Boot the emulator and sign requests for us-west-2.
2. create_keyspace(keyspaceName='mrks', replicationSpecification={'replicationStrategy':'MULTI_REGION','regionList':['us-west-2','eu-west-1']})
3. create_table(keyspaceName='mrks', tableName='tbl1', schemaDefinition={'allColumns':[{'name':'pk','type':'text'}],'partitionKeys':[{'name':'pk'}]}) and wait for status ACTIVE
4. get_table(keyspaceName='mrks', tableName='tbl1')
5. Observe that the response carries no `replicaSpecifications`; AWS returns one entry per replication Region (us-west-2, eu-west-1) with that Region's status and capacity summary.
6. get_table_auto_scaling_settings on the same table (created with PROVISIONED capacity + autoScalingSpecification) likewise reports no per-Region `replicaSpecifications`.
