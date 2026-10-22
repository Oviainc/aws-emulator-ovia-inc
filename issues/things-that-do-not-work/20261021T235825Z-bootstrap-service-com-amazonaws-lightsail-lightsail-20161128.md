Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: b135c4508c1fb26e9a8e4477f81bd51ec13345b2

# Bootstrap the Lightsail emulator

_Filed: 2026-10-21T23:58:25Z_
_Operation: service-com-amazonaws-lightsail-lightsail-20161128 | Class: stub_

> _Note: this issue was identified against the code base at commit `b135c4508c1fb26e9a8e4477f81bd51ec13345b2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 162 operations in `services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST, SMALL slice: the stateful lifecycle of Lightsail's primary resource, the instance (virtual private server).

Happy path to implement, in order (wire protocol: `awsJson1_1`, `POST /ls/api/2016-11-28/<OperationName>`, `X-Amz-Target: Lightsail_20161128.<OperationName>`):

1. **CreateInstances** — input `instanceNames` (list<string>, required), `availabilityZone` (required), `blueprintId` (required), `bundleId` (required), optional `keyPairName`, `userData`, `tags`, `ipAddressType`. Must INSERT one `Instance` record per name into the store and return `operations` (list<Operation>) with one real `Operation` per created instance (`id`, `resourceName` = the instance name, `resourceType` = `Instance`, `operationType` = `CreateInstance`, `status` = `Succeeded`/`Started`, `createdAt`, `isTerminal`). Creating a name that already exists must fail (`InvalidInputException`, HTTP 400), not silently overwrite.
2. **GetInstances** — input `pageToken`; output `instances` (list<Instance>), `nextPageToken`. Must enumerate the instances actually stored (empty list when nothing was created), not a canned list.
3. **GetInstance** — input `instanceName` (required); output `instance` (Instance). Must return the stored record with the SAME `name`, `blueprintId`, `bundleId`, `location.availabilityZone`/`regionName`, `tags`, `sshKeyName`, plus server-assigned `arn`, `createdAt`, `publicIpAddress`, `privateIpAddress`, `hardware`, `networking`, `resourceType` = `Instance`, and `state`. An unknown name must return `NotFoundException` (HTTP 404), never a zeroed 200.
4. **GetInstanceState** — input `instanceName` (required); output `state` (InstanceState with `code` + `name`). Must read the state field persisted by create/stop/start, i.e. `running` after create, `stopped` after StopInstance.
5. **StopInstance** / **StartInstance** — input `instanceName` (required, plus `force` for stop); output `operations`. Must MUTATE the stored instance's `state` (`stopped` / `running`) so step 4 and `GetInstance` observe the change, and record the operation so `GetInstance` stays consistent. Stopping/starting an unknown instance must return `NotFoundException` (HTTP 404).
6. **DeleteInstance** — input `instanceName` (required), `forceDeleteAddOns`; output `operations`. Must REMOVE the record so a subsequent `GetInstance` returns `NotFoundException` (HTTP 404) and `GetInstances` no longer lists it. Deleting an unknown instance must return `NotFoundException`.

Every one of these must compute its response from the persisted store reached through `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the request back, no generating an ARN/operation id without storing the resource. A create followed by a read must return what was created, and a delete must be observable by the next read. Each handler must also delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line at the top of its file.

## State Model

In `src/state.rs` (`ComAmazonawsLightsailLightsail20161128State`):

- `instances: BTreeMap<String /* instanceName */, Instance>` where `Instance { name, arn, support_code, created_at, location: ResourceLocation { availability_zone, region_name }, resource_type: "Instance", tags: Vec<Tag>, blueprint_id, blueprint_name, bundle_id, add_ons: Vec<AddOn>, is_static_ip: false, private_ip_address, public_ip_address, ipv6_addresses, ip_address_type, hardware: InstanceHardware { cpu_count, ram_size_in_gb, disks }, networking: InstanceNetworking { monthly_transfer, ports }, state: InstanceState { code, name }, username, ssh_key_name, metadata_options }` — populated from the CreateInstances input plus deterministic server-assigned values (arn built from the region/account and a generated uuid, `publicIpAddress` from a monotonic allocator, `username` derived from `blueprintId`).
- `operations: Vec<Operation>` (an append-only log, newest last) where `Operation { id, resource_name, resource_type, created_at, location, is_terminal, operation_details, operation_type, status, status_changed_at, error_code, error_details }` — appended by CreateInstances, StartInstance, StopInstance and DeleteInstance, and the source of truth for their `operations` output (and the basis for GetOperation/GetOperations in a later slice).
- `next_ip: u32` / an id counter used so public IPs and operation ids are unique per process.

All other operations (buckets, disks, domains, load balancers, relational databases, container services, distributions, snapshots, static IPs, alarms, metrics, tagging) stay stubbed at 501 and will be filed as individual gap issues later — do not widen this first slice.

## Evidence

`services/service-com-amazonaws-lightsail-lightsail-20161128/OPERATIONS.md` documents 162 operations from the Smithy model, including the input/output contracts cited above: CreateInstances (line 188: `instanceNames`/`availabilityZone`/`blueprintId`/`bundleId` required, output `operations` list<Operation>), GetInstance (line 671: in `instanceName`, out `instance`), GetInstances (line 699: in `pageToken`, out `instances` + `nextPageToken`), GetInstanceState (line 720: out `state` InstanceState), StartInstance (line 1028), StopInstance (line 1049: in `instanceName` + `force`), DeleteInstance (line 342: in `instanceName` + `forceDeleteAddOns`). All seven declare `NotFoundException (HTTP 404)` and `InvalidInputException (HTTP 400)`. The `Instance`, `InstanceState` and `Operation` shape members quoted in the State Model come from the Smithy model (`com.amazonaws.lightsail#Instance`, `#InstanceState`, `#Operation`). `grep -r '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 162 handler files — including `create_instances.rs`, `get_instances.rs`, `get_instance.rs`, `get_instance_state.rs`, `start_instance.rs`, `stop_instance.rs`, `delete_instance.rs` — each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501). No test asserts any of this behavior; the only test is the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. POST /ls/api/2016-11-28/CreateInstances with X-Amz-Target: Lightsail_20161128.CreateInstances and body {"instanceNames":["demo-1"],"availabilityZone":"us-east-1a","blueprintId":"amazon_linux_2023","bundleId":"micro_3_0"} — expect 200 with a non-empty `operations` list naming demo-1; currently 501 NotImplemented.
2. POST .../GetInstances with {} — expect `instances` to contain exactly one Instance named demo-1 with blueprintId `amazon_linux_2023` and bundleId `micro_3_0`.
3. POST .../GetInstance with {"instanceName":"demo-1"} — expect the stored instance echoed from state including arn, createdAt, location.availabilityZone = us-east-1a, publicIpAddress and state.name = "running".
4. POST .../GetInstanceState with {"instanceName":"demo-1"} — expect state.name = "running".
5. POST .../StopInstance with {"instanceName":"demo-1"}, then GetInstanceState again — expect state.name = "stopped"; then StartInstance and expect "running" again.
6. POST .../DeleteInstance with {"instanceName":"demo-1"} — expect 200 with `operations`; then GetInstance with the same name must return NotFoundException (HTTP 404) and GetInstances must return an empty `instances` list.
7. POST .../GetInstance with {"instanceName":"never-created"} — expect NotFoundException (HTTP 404), not a 501 and not a hollow 200.
