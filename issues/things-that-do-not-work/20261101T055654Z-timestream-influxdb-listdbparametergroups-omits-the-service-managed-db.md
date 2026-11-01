Issue-Source: trace-sweep-divergence
Issue-Trace: timestream-influxdb-catalog-list-db-parameter-groups
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: timestream-influxdb-catalog-list-db-parameter-groups
Issue-Shape-Id: com.amazonaws.timestreaminfluxdb#AmazonTimestreamInfluxDB
Issue-Key: timestream-influxdb|ListDbParameterGroups|2xx->2xx|$.items[*]
Issue-Commit: ac97ae6fa29734a328f1dd26f79054971c9b1584

# timestream-influxdb ListDbParameterGroups omits the service-managed DB parameter group catalog

> _Note: this issue was identified against the code base at commit `ac97ae6fa29734a328f1dd26f79054971c9b1584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDbParameterGroups

## Description
Cause 1 of 1: ListDbParameterGroups reports only customer-created parameter groups and never the service-managed (default) parameter-group catalog, so a fresh account lists nothing. Fixture timestream-influxdb-catalog-list-db-parameter-groups, step task#1.

AWS: ListDbParameterGroups is a read-only, paginated list of this account/region's DB parameter group summaries. Smithy (services/service-com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb/smithy-model/...json): ListDbParameterGroupsOutput.items -> DbParameterGroupSummaryList is @required; DbParameterGroupSummary requires id (DbParameterGroupId, len 3..64, pattern ^[a-zA-Z0-9]+$), name (DbParameterGroupName, len 3..64, pattern ^[a-zA-Z][a-zA-Z0-9]*(-[a-zA-Z0-9]+)*$) and arn (Arn), with optional description; input maxResults range 1..100, nextToken len 1.., paginated {inputToken: nextToken, outputToken: nextToken, items: items}. The model does not say what the list contains beyond "a list of Timestream for InfluxDB DB parameter group summaries", so for the catalog itself this is recorded behavior only: the recording shows a 200 whose items carries a set of service-managed groups that exist without anyone having created them. Those entries are the engine/instance-class default groups — ids/names such as InfluxDBV3Core (description "Service Parameter group to create InfluxDB v3 core engine"), InfluxDBV3Enterprise ("Service Parameter group to create InfluxDB v3 enterprise engine"), InfluxDBV3Enterprise1Node, and the per-instance-size service-managed defaults InfluxDBV3Core<Size>Default with description "Service-managed parameter group: InfluxDBV3Core<Size>Default" (Medium, Large, Xlarge, 2xlarge, 4xlarge, 8xlarge, 12xlarge, ...), 21 entries in all in the recording. In the recorded response these service-managed summaries carry id == name == the group name and an EMPTY arn (""), i.e. they are not account-scoped resources. Fixed text of each description is the "Service Parameter group to create InfluxDB v3 <engine> engine" / "Service-managed parameter group: " prefix; the variable part is the group id/name it names. The specific set of group names/sizes and the count are the catalog's content, not one run's values.

Emulator: answers HTTP 200 but with an empty items list. The replay reports $.items[0] .. $.items[20] all "missing ... got None" against the 21 expected summaries, i.e. nothing is listed at all. The cause is in the handler and state: services/.../src/ops/list_db_parameter_groups.rs builds items only by ranging over state.db_parameter_groups (lines 181-186), and ComAmazonawsTimestreaminfluxdbAmazontimestreaminfluxdbState (src/state.rs:325-347) derives Default with db_parameter_groups empty, so no service-managed catalog is ever seeded.

Why: parameter-group discovery is how a client chooses the group to pass as CreateDbInstance/CreateDbCluster dbParameterGroupIdentifier, and on AWS the v3 core/enterprise and per-size service-managed defaults are always available. Against the emulator the list is empty, so a client that enumerates the catalog to pick a default (or that asserts a known default such as InfluxDBV3Core exists) finds no candidate and either fails outright or falls back to creating its own group. A paginating client also sees a page count of zero where AWS returns a full first page, so pagination over the catalog is never exercised.

Fix: the operation's state — seed the service-managed DB parameter group catalog into the service state (src/state.rs, alongside db_parameter_groups) and have ListDbParameterGroups report it ahead of / merged with the customer-created groups in a stable order, keeping the existing maxResults/nextToken paging over the combined set (src/ops/list_db_parameter_groups.rs handle). GetDbParameterGroup should resolve the same service-managed identifiers rather than 404 on them, and the seeded entries must keep the recorded summary shape (id == name, empty arn, the service-managed description).

Note: the open issues for this service (CreateDbParameterGroup ConflictException message, TagResource stub) concern other operations and do not cover this catalog gap.

Traces: timestream-influxdb-catalog-list-db-parameter-groups#task#1.

## Evidence
Fixture timestream-influxdb-catalog-list-db-parameter-groups, step task#1, operation ListDbParameterGroups, verdict mismatch. Expected status 200, actual status 200, but the body diverges on every element of the required items list:
  $.items[0]: missing (expected {'arn': '', 'description': 'Service Parameter group to create InfluxDB v3 core engine', 'id': 'InfluxDBV3Core', 'name': 'InfluxDBV3Core'}, got None)
  $.items[1]: missing (expected {'arn': '', 'description': 'Service Parameter group to create InfluxDB v3 enterprise engine', 'id': 'InfluxDBV3Enterprise', 'name': 'InfluxDBV3Enterprise'}, got None)
  $.items[2]: missing (expected {'arn': '', 'description': 'Service Parameter group to create InfluxDB v3 enterprise engine with 1 node', 'id': 'InfluxDBV3Enterprise1Node', 'name': 'InfluxDBV3Enterprise1Node'}, got None)
  $.items[3..9]: missing (expected the service-managed defaults InfluxDBV3CoreMediumDefault, InfluxDBV3CoreLargeDefault, InfluxDBV3CoreXlargeDefault, InfluxDBV3Core2xlargeDefault, InfluxDBV3Core4xlargeDefault, InfluxDBV3Core8xlargeDefault, InfluxDBV3Core12xlargeDefault, each with description 'Service-managed parameter group: <id>' and arn '')
  divergence_key lists $.items[0] .. $.items[20] — all 21 expected summaries are absent.
Emulator answered an empty items array. Code: src/ops/list_db_parameter_groups.rs:181-186 lists only state.db_parameter_groups; src/state.rs:325-347 starts that map empty (#[derive(Default)]).

## Steps to Reproduce
1. Boot the emulator from this checkout (timestream-influxdb is bootstrapped and answers).
2. Against a fresh, empty state call ListDbParameterGroups with no maxResults and no nextToken (POST /.../ListDbParameterGroups per the service's REST-JSON binding), as fixture timestream-influxdb-catalog-list-db-parameter-groups step task#1 does.
3. Observe HTTP 200 with "items": [].
4. AWS returns 200 with the service-managed parameter-group catalog: InfluxDBV3Core, InfluxDBV3Enterprise, InfluxDBV3Enterprise1Node and the per-size InfluxDBV3Core<Size>Default groups (21 summaries in the recording), each with id == name, arn "" and its service-managed description.
5. Also call GetDbParameterGroup with identifier=InfluxDBV3Core: AWS resolves the service-managed group, the emulator does not know it.
