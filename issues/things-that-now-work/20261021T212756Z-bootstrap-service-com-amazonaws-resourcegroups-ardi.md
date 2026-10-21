Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the Resource Groups emulator

_Filed: 2026-10-21T21:27:56Z_
_Operation: service-com-amazonaws-resourcegroups-ardi | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 23 operations in `services/service-com-amazonaws-resourcegroups-ardi/src/ops/` is an unmodified scaffold: each file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 `NotImplemented`, and the service holds no state. No client workflow works at all.

Implement a FIRST, SMALL, stateful slice: the resource-group lifecycle plus static membership. Wire protocol is `restJson1`, sigv4 signing name `resource-groups`.

Happy path to implement, in order:

1. **CreateGroup** (`POST /groups`) — input `Name` (required, 1..300, `^[a-zA-Z0-9_\.-]+$`), `Description`, `ResourceQuery` (`ResourceQuery{Type: CLOUDFORMATION_STACK_1_0|TAG_FILTERS_1_0, Query}`), `Tags` (map), `Criticality` (1..10), `Owner`, `DisplayName`. Must persist a new group and return `Group` (`Group{GroupArn, Name, Description, Criticality, Owner, DisplayName}`), plus the `ResourceQuery` and `Tags` as stored. `GroupArn` must be synthesized as `arn:aws:resource-groups:<region>:<account>:group/<Name>` so it matches the modeled ARN pattern, and must be stable across later calls. Creating a group whose `Name` already exists must fail `BadRequestException` (HTTP 400), not overwrite.

2. **GetGroup** (`POST /get-group`) — input `GroupName` or `Group` (name or ARN; resolve both forms). Must return the stored `Group` structure exactly as created/updated. Unknown group -> `NotFoundException` (HTTP 404).

3. **ListGroups** (`POST /groups-list`) — must return one `GroupIdentifier{GroupName, GroupArn, Description, Criticality, Owner, DisplayName}` per persisted group in `GroupIdentifiers` (and the same groups in `Groups`), computed from the store — never a canned list. Honour `MaxResults` (query param `maxResults`, 1..50) and round-trip `NextToken` (query param `nextToken`) so a second page continues where the first stopped, and a deleted group disappears from the listing.

4. **GroupResources** (`POST /group-resources`) — input `Group` (required) and `ResourceArns` (required, 1..10). Must record each ARN as a member of that group and return the accepted ARNs in `Succeeded` (with `Failed`/`Pending` empty on the happy path). Unknown group -> `NotFoundException` (404).

5. **ListGroupResources** (`POST /list-group-resources`) — must return exactly the ARNs previously added via GroupResources (minus any removed), as `ResourceIdentifiers` (`ResourceIdentifier{ResourceArn, ResourceType}`) and `Resources` (`ListGroupResourcesItem{Identifier, Status}`), honouring `MaxResults`/`NextToken`. Unknown group -> `NotFoundException` (404).

6. **DeleteGroup** (`POST /delete-group`) — input `GroupName` or `Group`. Must remove the group (and its membership and tags) from the store and return the deleted `Group`. A subsequent GetGroup must return `NotFoundException` (404) and ListGroups must no longer contain it. Deleting an unknown group -> `NotFoundException` (404).

Hard requirements:
- Every response must be computed from persisted state held in the ONE shared engine reached via `crate::state::service_state(&*self.engine)` (see README). No canned responses, no echoing the input back, no generated ARN that is not stored.
- A create followed by a read must return what was created, including `Description`, `Criticality`, `Owner`, `DisplayName` and `ResourceQuery`.
- Errors must use the modeled shapes/statuses: `BadRequestException` 400, `NotFoundException` 404.
- Required-field and constraint violations (missing `Name`, `Name` failing the pattern, `ResourceArns` empty or >10, `MaxResults` outside 1..50, `Criticality` outside 1..10) must return `BadRequestException` 400, not a 2xx.
- Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line from each of the six files as it is implemented (that marker is the stub census).

## State Model

In `src/state.rs`, on `ComAmazonawsResourcegroupsArdiState`:

- `groups: BTreeMap<String /* group name */, GroupRecord>` — ordered so ListGroups paging is deterministic.
- `GroupRecord {
    name: String,
    group_arn: String,                     // arn:aws:resource-groups:<region>:<account>:group/<name>
    description: Option<String>,
    criticality: Option<i32>,
    owner: Option<String>,
    display_name: Option<String>,
    resource_query: Option<ResourceQueryRecord>,
    tags: BTreeMap<String, String>,
    members: Vec<MemberRecord>,            // populated by GroupResources, drained by UngroupResources
    created_at: SystemTime,
  }`
- `ResourceQueryRecord { query_type: String /* CLOUDFORMATION_STACK_1_0 | TAG_FILTERS_1_0 */, query: String }`
- `MemberRecord { resource_arn: String, resource_type: Option<String> /* derived from the ARN service+type, e.g. AWS::EC2::Instance */, status: Option<String> }`
- An ARN index or helper `fn resolve_group(&self, group_name: Option<&str>, group: Option<&str>) -> Option<&GroupRecord>` so `GroupName`, a bare name, and a full `GroupArn` all resolve to the same record.
- Pagination tokens: opaque base64 strings matching `^[a-zA-Z0-9+/]*={0,2}$` encoding the offset/last-key into `groups` or `members`.

Out of scope for this first slice (stay 501, filed separately later): GetGroupConfiguration, PutGroupConfiguration, GetGroupQuery, UpdateGroup, UpdateGroupQuery, UngroupResources, SearchResources, Tag, Untag, GetTags, ListGroupingStatuses, GetAccountSettings, UpdateAccountSettings, StartTagSyncTask, GetTagSyncTask, ListTagSyncTasks, CancelTagSyncTask.

## Evidence

OPERATIONS.md (crate root) lists all 23 operations with routes, inputs, outputs and errors: CreateGroup `POST /groups` -> `Group`/`ResourceQuery`/`Tags`/`GroupConfiguration`; GetGroup `POST /get-group` -> `Group` with `NotFoundException (HTTP 404)`; ListGroups `POST /groups-list` @paginated(inputToken=NextToken, outputToken=NextToken, items=GroupIdentifiers) with `MaxResults` @httpQuery(maxResults); GroupResources `POST /group-resources` with required `Group` + `ResourceArns` (len 1..10) -> `Succeeded`/`Failed`/`Pending`; ListGroupResources `POST /list-group-resources` @paginated(items=ResourceIdentifiers); DeleteGroup `POST /delete-group` -> `Group`. Smithy model shapes confirm `Group{GroupArn (required), Name (required), Description, Criticality, Owner, DisplayName, ApplicationTag}`, `GroupIdentifier{GroupName, GroupArn, Description, Criticality, Owner, DisplayName}`, `ResourceIdentifier{ResourceArn, ResourceType}`, `ListGroupResourcesItem{Identifier, Status}`, `ResourceQuery{Type (required), Query (required)}`. Stub census: `grep -n 'SCAFFOLDED, NOT IMPLEMENTED' services/service-com-amazonaws-resourcegroups-ardi/src/ops/` matches every operation file including create_group.rs, get_group.rs, list_groups.rs, group_resources.rs, list_group_resources.rs and delete_group.rs. No tests exist beyond the generated `tests/state_roundtrip.rs` example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `resource-groups` at it.
2. CreateGroup Name=demo-group Description='demo group' ResourceQuery='{"Type":"TAG_FILTERS_1_0","Query":"{\"ResourceTypeFilters\":[\"AWS::AllSupported\"],\"TagFilters\":[{\"Key\":\"Stage\",\"Values\":[\"Test\"]}]}"}' — today: HTTP 501 NotImplemented; expected: 200 with Group.Name=demo-group and a well-formed Group.GroupArn ending in :group/demo-group.
3. GetGroup GroupName=demo-group — expected 200 returning the same GroupArn and Description that CreateGroup returned.
4. ListGroups — expected 200 with GroupIdentifiers containing exactly demo-group.
5. GroupResources Group=demo-group ResourceArns=['arn:aws:ec2:us-east-1:123456789012:instance/i-0123456789abcdef0'] — expected 200 with that ARN in Succeeded.
6. ListGroupResources GroupName=demo-group — expected 200 with ResourceIdentifiers containing exactly that instance ARN.
7. DeleteGroup GroupName=demo-group — expected 200 returning the deleted Group.
8. GetGroup GroupName=demo-group again — expected NotFoundException with HTTP 404; ListGroups must now return an empty GroupIdentifiers list.
