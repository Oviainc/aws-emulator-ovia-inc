Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the drs (Elastic Disaster Recovery) emulator

_Filed: 2026-10-21T21:58:40Z_
_Operation: service-com-amazonaws-drs-elasticdisasterrecoveryservice | Class: missing_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 70 operations in `services/service-com-amazonaws-drs-elasticdisasterrecoveryservice/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, and the crate holds no state. The service is therefore unusable by any client.

This issue defines the FIRST, SMALL slice to implement: the account-initialization + ReplicationConfigurationTemplate lifecycle, which is the only primary DRS resource whose full create -> read -> update -> delete cycle is reachable through the public API (source servers and recovery instances are created by agents/launches, not by a CreateX call). Protocol is `restJson1`, SDK id `drs`, signing name `drs`; each operation is POST to `/<OperationName>` (per OPERATIONS.md 'Wire protocol').

Implement these 5 operations, in this order, each computing its response from persisted state (no canned bodies, no echoing the input without storing it, no random ID that is never recorded):

1. **InitializeService** — `POST /InitializeService -> 204`, no input members, no output members. Must flip a persisted `initialized` flag for the emulated account. Before it has been called, the other four operations must fail with `UninitializedAccountException` (HTTP 400), which every one of them models; after it, they must succeed. Calling it twice is a no-op 204.

2. **CreateReplicationConfigurationTemplate** — `POST /CreateReplicationConfigurationTemplate -> 201`. Required input: `stagingAreaSubnetId` (`^subnet-[0-9a-fA-F]{8,}$`), `replicationServersSecurityGroupsIDs`, `ebsEncryption` (CUSTOM|DEFAULT|NONE), `bandwidthThrottling` (long, default 0), `stagingAreaTags`, `pitPolicy` (exactly 3 rules); optional: `associateDefaultSecurityGroup`, `replicationServerInstanceType`, `useDedicatedReplicationServer`, `defaultLargeStagingDiskType`, `ebsEncryptionKeyArn`, `dataPlaneRouting`, `createPublicIP`, `tags`, `autoReplicateNewDisks`, `internetProtocol`. Must mint a fresh `replicationConfigurationTemplateID` matching `^rct-[0-9a-zA-Z]{17}$` (21 chars) plus an `arn` of the form `arn:aws:drs:<region>:<account>:replication-configuration-template/<id>`, store EVERY supplied field under that ID, and return the stored record (not a zeroed struct).

3. **DescribeReplicationConfigurationTemplates** — `POST /DescribeReplicationConfigurationTemplates -> 200`, `@paginated(inputToken=nextToken, outputToken=nextToken, items=items)`, `@readonly`. Input `replicationConfigurationTemplateIDs` (0..200), `maxResults` (>=1), `nextToken`. Must return the templates actually in the store: empty `items` on a fresh account, the created template with every field exactly as created, filtering by the ID list when given, honouring `maxResults` and returning/accepting an opaque `nextToken` that resumes where the previous page stopped. Must not mutate state.

4. **UpdateReplicationConfigurationTemplate** — `POST /UpdateReplicationConfigurationTemplate -> 200`. Required `replicationConfigurationTemplateID`; the other members are the mutable fields. Must merge the supplied fields into the STORED template (leaving unspecified fields unchanged), return the merged record, and make the change visible to a following Describe. An unknown ID must yield `ResourceNotFoundException` (HTTP 404).

5. **DeleteReplicationConfigurationTemplate** — `POST /DeleteReplicationConfigurationTemplate -> 204`, `@idempotent`, required `replicationConfigurationTemplateID`, empty output. Must remove the template from the store so a following Describe of that ID returns `ResourceNotFoundException` (404) and an unfiltered Describe no longer lists it.

Also in scope as a read of the same store: **DescribeSourceServers** (`POST /DescribeSourceServers -> 200`, paginated, `@readonly`) must answer `200` with an empty `items` list on an initialized account rather than 501, so SDK clients and the console-style "is DRS set up?" probe work.

## State Model

In `src/state.rs` (`ComAmazonawsDrsElasticdisasterrecoveryserviceState`, reached via `crate::state::service_state(&*self.engine)`):

- `initialized: bool` — set by InitializeService; gates `UninitializedAccountException`.
- `replication_configuration_templates: IndexMap<String /* rct-id */, ReplicationConfigurationTemplateRecord>` — insertion-ordered so pagination is stable.
- `ReplicationConfigurationTemplateRecord { id: String, arn: String, staging_area_subnet_id: String, associate_default_security_group: Option<bool>, replication_servers_security_groups_ids: Vec<String>, replication_server_instance_type: Option<String>, use_dedicated_replication_server: Option<bool>, default_large_staging_disk_type: Option<String>, ebs_encryption: String, ebs_encryption_key_arn: Option<String>, bandwidth_throttling: i64, data_plane_routing: Option<String>, create_public_ip: Option<bool>, staging_area_tags: HashMap<String,String>, tags: HashMap<String,String>, pit_policy: Vec<PitPolicyRule>, auto_replicate_new_disks: Option<bool>, internet_protocol: Option<String> }`
- `source_servers: IndexMap<String /* s-id */, SourceServerRecord>` — empty for now; DescribeSourceServers reads it.
- `id_counter: u64` (or an RNG) used ONLY to mint `rct-`/`s-` suffixes that are then recorded in the maps above.

## Done when

A realistic client script (AWS SDK or raw restJson1 POSTs) runs the workflow below end to end with correct, stateful bodies — not merely 2xx. Every other DRS operation (recovery plans, jobs, launch configurations, source networks, failback, tagging, StartRecovery, ...) stays 501 and will be filed as individual gap issues later.

## Evidence

OPERATIONS.md for this crate: 'Wire protocol: restJson1, SDK id drs'; `## InitializeService` (POST /InitializeService -> 204, no members, errors include ValidationException); `## CreateReplicationConfigurationTemplate` (POST -> 201, required stagingAreaSubnetId/replicationServersSecurityGroupsIDs/ebsEncryption/bandwidthThrottling/stagingAreaTags/pitPolicy, output replicationConfigurationTemplateID `^rct-[0-9a-zA-Z]{17}$` + arn, errors UninitializedAccountException 400, ValidationException 400); `## DescribeReplicationConfigurationTemplates` (POST -> 200, @paginated(inputToken=nextToken, outputToken=nextToken, items=items), @readonly, errors ResourceNotFoundException 404); `## UpdateReplicationConfigurationTemplate` (POST -> 200, required ID, errors ResourceNotFoundException 404); `## DeleteReplicationConfigurationTemplate` (POST -> 204, @idempotent, empty output, errors ResourceNotFoundException 404, ConflictException 409); `## DescribeSourceServers` (POST -> 200, @paginated, @readonly).

Handler proof: `src/ops/create_replication_configuration_template.rs` lines 4 and 234 — `//! SCAFFOLDED, NOT IMPLEMENTED:` and `Err(crate::scaffolded::Scaffolded::not_implemented("CreateReplicationConfigurationTemplate"))`; all 70 files in `src/ops/` carry the same marker, so no operation holds or reads state. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. POST /DescribeReplicationConfigurationTemplates with `{}` against a fresh emulator — expect HTTP 400 `UninitializedAccountException`; actual: HTTP 501 NotImplemented.
2. POST /InitializeService with `{}` — expect HTTP 204 and the account marked initialized; actual: 501.
3. POST /DescribeReplicationConfigurationTemplates with `{}` — expect HTTP 200 and `{"items":[]}`.
4. POST /CreateReplicationConfigurationTemplate with `{"stagingAreaSubnetId":"subnet-0123456789abcdef0","replicationServersSecurityGroupsIDs":["sg-0123456789abcdef0"],"ebsEncryption":"DEFAULT","bandwidthThrottling":0,"stagingAreaTags":{"env":"test"},"pitPolicy":[{"ruleID":1,"units":"MINUTE","interval":10,"retentionDuration":60,"enabled":true},{"ruleID":2,"units":"HOUR","interval":1,"retentionDuration":24,"enabled":true},{"ruleID":3,"units":"DAY","interval":1,"retentionDuration":7,"enabled":true}],"tags":{"team":"dr"}}` — expect HTTP 201 with a `replicationConfigurationTemplateID` matching ^rct-[0-9a-zA-Z]{17}$ and every submitted field echoed from the stored record.
5. POST /DescribeReplicationConfigurationTemplates with `{"replicationConfigurationTemplateIDs":["<rct-id>"]}` — expect HTTP 200, one item, fields identical to what was created (proves persistence, not echo).
6. POST /UpdateReplicationConfigurationTemplate with `{"replicationConfigurationTemplateID":"<rct-id>","bandwidthThrottling":500,"defaultLargeStagingDiskType":"GP3"}` — expect HTTP 200 with the merged record; re-Describe must show bandwidthThrottling=500, GP3, and the original stagingAreaSubnetId/tags unchanged.
7. POST /UpdateReplicationConfigurationTemplate with an unknown `rct-00000000000000000` — expect HTTP 404 `ResourceNotFoundException`.
8. POST /DescribeReplicationConfigurationTemplates with `{"maxResults":1}` after creating two templates — expect one item plus a `nextToken`; re-POST with that token to get the second item and no token.
9. POST /DeleteReplicationConfigurationTemplate with `{"replicationConfigurationTemplateID":"<rct-id>"}` — expect HTTP 204 and an empty body.
10. POST /DescribeReplicationConfigurationTemplates with `{"replicationConfigurationTemplateIDs":["<rct-id>"]}` — expect HTTP 404 `ResourceNotFoundException` (template gone); an unfiltered Describe must no longer list it.
11. POST /DescribeSourceServers with `{}` on the initialized account — expect HTTP 200 `{"items":[]}`; actual: 501.
