Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the mediaconnect emulator

_Filed: 2026-10-21T21:48:19Z_
_Operation: service-com-amazonaws-mediaconnect-mediaconnect | Class: missing_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 82 operations in `services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no MediaConnect client workflow works at all. Implement a FIRST SLICE: the stateful lifecycle of the service's primary resource, the **flow** (protocol `restJson1`, sigv4 name `mediaconnect`).

### Happy path to implement (in order)

1. **CreateFlow** — `POST /v1/flows` -> 201. Input: `Name` (required), `Source` (SetSourceRequest) or `Sources`, `Outputs` (list<AddOutputRequest>), `AvailabilityZone`, `Entitlements`, `MediaStreams`, `VpcInterfaces`, `Maintenance`, `FlowSize`, `SourceFailoverConfig`. Output: `Flow`. Must mint a FlowArn matching the modeled pattern `^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:flow:[a-zA-Z0-9-]+:[a-zA-Z0-9_-]+$` (e.g. `arn:aws:mediaconnect:us-east-1:000000000000:flow:1-abcdef12:<Name>`), materialize each requested source into a `Source` (own `SourceArn`, `Name`, `IngestIp`, `IngestPort`, `Transport` derived from the request's protocol/whitelistCidr/maxBitrate/maxLatency) and each requested output into an `Output` (own `OutputArn`, `Name`, `Destination`, `Port`, `Transport`), set `Status = STANDBY`, default `AvailabilityZone` when omitted, and PERSIST the whole flow. The returned `Flow` must be the stored record, not an echo of the request body.
2. **DescribeFlow** — `GET /v1/flows/{FlowArn}` -> 200. Input: `FlowArn` (required, httpLabel). Output: `Flow`, `Messages`. Must look the flow up in state and return exactly what was created/updated (same Name, Source(s) with their generated SourceArn/IngestIp/IngestPort, Outputs with their OutputArns, current `Status`). Unknown ARN -> `NotFoundException` (HTTP 404); ARN failing the pattern -> `BadRequestException` (HTTP 400).
3. **ListFlows** — `GET /v1/flows` -> 200. Input: `MaxResults` (1..1000, query `maxResults`), `NextToken` (query `nextToken`). Output: `Flows` (list<ListedFlow>: `FlowArn`, `Name`, `Description`, `AvailabilityZone`, `SourceType`, `Status`, `Maintenance`), `NextToken`. Must enumerate the persisted flows, honour `MaxResults` and return/accept a real `NextToken` (paginated trait: inputToken=NextToken, outputToken=NextToken, items=Flows). A deleted flow must not appear.
4. **StartFlow** — `POST /v1/flows/start/{FlowArn}` -> 202. Output: `FlowArn`, `Status`. Must transition the persisted flow's status to `STARTING`/`ACTIVE` so a following DescribeFlow reports the new status; unknown ARN -> 404.
5. **StopFlow** — `POST /v1/flows/stop/{FlowArn}` -> 202. Output: `FlowArn`, `Status`. Must transition the persisted flow to `STOPPING`/`STANDBY`, observable via DescribeFlow; unknown ARN -> 404.
6. **DeleteFlow** — `DELETE /v1/flows/{FlowArn}` -> 202. Output: `FlowArn`, `Status` (`DELETING`). Must remove the flow from state so DescribeFlow afterwards returns `NotFoundException` (404) and ListFlows no longer lists it.

NEVER STUBS: each of these six must compute its response from the persisted store reached through `crate::state::service_state(&*self.engine)` (never a fresh engine) — no hardcoded ARNs-without-storage, no echoing the input back, no canned `Flow` literal. Done = a client script can run CreateFlow -> DescribeFlow -> ListFlows -> StartFlow -> StopFlow -> DeleteFlow -> DescribeFlow(404) with response bodies that agree with each other.

## State Model

In `src/state.rs` (the service's locked state struct):

- `flows: BTreeMap<String /* FlowArn */, StoredFlow>` — insertion/name-ordered map so ListFlows is stable and paginable.
- `flow_arns_by_name: BTreeMap<String /* Name */, String /* FlowArn */>` — to reject duplicate flow names and to look flows up by name.
- `StoredFlow { flow_arn: String, name: String, description: Option<String>, availability_zone: String, status: Status /* ACTIVE|DELETING|ERROR|STANDBY|STARTING|STOPPING|UPDATING */, egress_ip: Option<String>, flow_size: Option<FlowSize>, maintenance: Option<Maintenance>, source_failover_config: Option<FailoverConfig>, source_monitoring_config: Option<MonitoringConfig>, sources: Vec<StoredSource>, outputs: Vec<StoredOutput>, entitlements: Vec<StoredEntitlement>, media_streams: Vec<MediaStream>, vpc_interfaces: Vec<VpcInterface>, tags: BTreeMap<String,String>, created_at: SystemTime }`
- `StoredSource { source_arn: String, name: String, description: Option<String>, ingest_ip: String, ingest_port: Option<i32>, whitelist_cidr: Option<String>, protocol: Option<Protocol>, max_bitrate: Option<i32>, max_latency: Option<i32>, entitlement_arn: Option<String>, vpc_interface_name: Option<String>, transport: Option<Transport> }` (the `Flow.Source` member is the first entry; `Flow.Sources` is the whole vec).
- `StoredOutput { output_arn: String, name: String, description: Option<String>, destination: Option<String>, port: Option<i32>, protocol: Option<Protocol>, encryption: Option<Encryption>, cidr_allow_list: Vec<String>, output_status: Option<OutputStatus>, transport: Option<Transport> }`
- `next_arn_suffix: u64` — monotonic counter used to mint unique flow/source/output ARN fragments.

All other MediaConnect operations (bridges, gateways, router inputs/outputs, reservations/offerings, entitlement grants, tagging, AddFlowOutputs/UpdateFlow*/RemoveFlow*) stay 501 for now and are tracked as separate gap issues.

## Evidence

`services/service-com-amazonaws-mediaconnect-mediaconnect/OPERATIONS.md` lists 82 operations for sdkId `MediaConnect`, protocol `restJson1`, and gives the exact contracts cited above: CreateFlow (`POST /v1/flows` -> 201, `Name` required, output `Flow`), DescribeFlow (`GET /v1/flows/{FlowArn}` -> 200, output `Flow`+`Messages`, errors include NotFoundException 404 / BadRequestException 400), ListFlows (`GET /v1/flows` -> 200, @paginated items=Flows, MaxResults range 1..1000), StartFlow (`POST /v1/flows/start/{FlowArn}` -> 202, output `FlowArn`+`Status`), StopFlow (`POST /v1/flows/stop/{FlowArn}` -> 202), DeleteFlow (`DELETE /v1/flows/{FlowArn}` -> 202, output `FlowArn`+`Status`). The `Flow`, `ListedFlow` and `Source` shapes in the Smithy model supply the required members used in the state model (Flow.FlowArn/Name/Status/Source/Outputs/Entitlements/AvailabilityZone required; Source.SourceArn/Name required, plus IngestIp/IngestPort/Transport). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/` matches every handler file, including create_flow.rs, describe_flow.rs, list_flows.rs, start_flow.rs, stop_flow.rs and delete_flow.rs — all return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501). No integration test exercises any flow operation.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI mediaconnect client at it.
2. `aws mediaconnect create-flow --name demo-flow --source Name=src1,Protocol=rtp,WhitelistCidr=10.0.0.0/16` — expect 201 with a Flow carrying a generated FlowArn, a Source with its own SourceArn/IngestIp/IngestPort and Status=STANDBY; today: HTTP 501 NotImplemented.
3. `aws mediaconnect describe-flow --flow-arn <arn from step 2>` — expect 200 returning the same flow (same name, same SourceArn, Status=STANDBY).
4. `aws mediaconnect list-flows --max-results 10` — expect the new flow in `Flows` with its FlowArn, Name, AvailabilityZone, SourceType and Status.
5. `aws mediaconnect start-flow --flow-arn <arn>` — expect 202 with Status STARTING/ACTIVE, then describe-flow must report the started status.
6. `aws mediaconnect stop-flow --flow-arn <arn>` — expect 202 with Status STOPPING/STANDBY, then describe-flow must report it.
7. `aws mediaconnect delete-flow --flow-arn <arn>` — expect 202 with Status DELETING.
8. `aws mediaconnect describe-flow --flow-arn <arn>` — expect NotFoundException (HTTP 404), and `list-flows` must no longer include the flow.
9. `aws mediaconnect describe-flow --flow-arn arn:aws:mediaconnect:us-east-1:000000000000:flow:1-deadbeef:missing` — expect NotFoundException (404), not 501.
