Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the networkmonitor emulator

_Filed: 2026-10-21T23:18:06Z_
_Operation: service-com-amazonaws-networkmonitor-networkmonitor | Class: missing_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 12 operations in `services/service-com-amazonaws-networkmonitor-networkmonitor/src/ops/` is an unmodified scaffold: the handler carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, holding zero state. CloudWatch Network Monitor's primary resource is the Monitor, which contains Probes, so the first buildable slice is the monitor lifecycle plus one probe.

Implement exactly these 6 operations, statefully, in this order (restJson1, sigv4 signing name `networkmonitor`):

1. **CreateMonitor** — `POST /monitors`. Input: `monitorName` (required, 1..200, `^[a-zA-Z0-9_-]+$`), optional `probes` (list<CreateMonitorProbeInput>), `aggregationPeriod` (long, >=30; 60 is the documented default when omitted), `clientToken` (@idempotencyToken), `tags`. Must allocate and persist a monitor record, mint `monitorArn` of the form `arn:aws:networkmonitor:<region>:<account>:monitor/<monitorName>`, set `state` (PENDING for a monitor with no probes, ACTIVE once probes exist is acceptable — pick one and be consistent), stamp `createdAt`/`modifiedAt`, and create a Probe record for each entry in `probes`. Return `monitorArn`, `monitorName`, `state`, `aggregationPeriod`, `tags` computed from the stored record. A second CreateMonitor with the same `monitorName` must fail `ConflictException` (409), not succeed.
2. **CreateProbe** — `POST /monitors/{monitorName}/probes`. Input: `monitorName` (@httpLabel), `probe` (ProbeInput, required: sourceArn, destination, protocol, and destinationPort/packetSize/addressFamily), optional `clientToken`, `tags`. Must mint a `probeId` matching `^probe-[a-z0-9A-Z-]{21,64}$` and a `probeArn`, attach the probe to the stored monitor, stamp `createdAt`/`modifiedAt`, and return every echoed field FROM the stored probe. Unknown `monitorName` must return `ResourceNotFoundException` (404).
3. **GetMonitor** — `GET /monitors/{monitorName}`. Must return the persisted monitor: `monitorArn`, `monitorName`, `state`, `aggregationPeriod`, `tags`, `probes` (list<Probe>, including the probe created in step 2 with its real probeId/state), `createdAt`, `modifiedAt`. Unknown name => `ResourceNotFoundException` (404).
4. **ListMonitors** — `GET /monitors`. Must list MonitorSummary entries built from stored monitors, honour `maxResults` (1..25) and the opaque `nextToken` round trip (@paginated inputToken=nextToken, outputToken=nextToken, items=monitors), and filter by the `state` query parameter when present. `nextToken` must be absent on the final page.
5. **DeleteProbe** — `DELETE /monitors/{monitorName}/probes/{probeId}`. Must remove the probe from the stored monitor so a following GetMonitor no longer lists it; unknown monitor or probe => `ResourceNotFoundException` (404).
6. **DeleteMonitor** — `DELETE /monitors/{monitorName}`. Must remove the monitor (and its probes) from state so GetMonitor afterwards returns `ResourceNotFoundException` (404) and ListMonitors no longer includes it. Unknown name => 404.

NEVER STUBS: each of the six must compute its response from persisted state. Hardcoded responses, echoing the request body back without storing it, or minting an id without persisting the record are failures of this issue. 501 is still the correct answer for the other six operations (UpdateMonitor, GetProbe, UpdateProbe, TagResource, UntagResource, ListTagsForResource) — they will be filed as individual gap issues later.

## State Model

Service state (reached via `crate::state::service_state(&*self.engine)`, behind the existing lock):

```text
NetworkMonitorState {
    monitors: BTreeMap<String /* monitorName */, Monitor>,
    // optional: idempotency cache clientToken -> monitorName / probeId
    client_tokens: HashMap<String, String>,
}

Monitor {
    monitor_name: String,
    monitor_arn: String,          // arn:aws:networkmonitor:<region>:<account>:monitor/<name>
    state: MonitorState,          // ACTIVE | DELETING | ERROR | INACTIVE | PENDING
    aggregation_period: i64,      // 30 or 60, default 60
    tags: BTreeMap<String, String>,
    probes: BTreeMap<String /* probeId */, Probe>,
    created_at: DateTime,
    modified_at: DateTime,
}

Probe {
    probe_id: String,             // ^probe-[a-z0-9A-Z-]{21,64}$
    probe_arn: String,
    source_arn: String,
    destination: String,
    destination_port: Option<i32>, // 0..65536, required for TCP
    protocol: Protocol,            // ICMP | TCP
    packet_size: Option<i32>,      // 56..8500
    address_family: Option<AddressFamily>, // IPV4 | IPV6
    vpc_id: Option<String>,        // ^vpc-[a-zA-Z0-9]{8,32}$
    state: ProbeState,             // ACTIVE | DELETED | DELETING | ERROR | INACTIVE | PENDING
    tags: BTreeMap<String, String>,
    created_at: DateTime,
    modified_at: DateTime,
}
```

ListMonitors' `nextToken` should be an opaque encoding of the last-returned monitorName so paging is stable against the BTreeMap ordering.

Done = a client script can run CreateMonitor -> CreateProbe -> GetMonitor -> ListMonitors -> DeleteProbe -> DeleteMonitor and observe correct, stateful bodies at every step (not merely 2xx).

## Evidence

OPERATIONS.md at the crate root (`services/service-com-amazonaws-networkmonitor-networkmonitor/OPERATIONS.md`) lists all 12 operations with routes, input/output members and errors: CreateMonitor `POST /monitors` (lines 49-55), GetMonitor `GET /monitors/{monitorName}` with required `createdAt`/`modifiedAt`/`probes` (lines 57-63), ListMonitors `GET /monitors` with @paginated(inputToken=nextToken, outputToken=nextToken, items=monitors) and maxResults range 1..25 (lines 81-87), CreateProbe `POST /monitors/{monitorName}/probes` with probeId pattern `^probe-[a-z0-9A-Z-]{21,64}$` (lines 89-95), DeleteProbe (lines 113-119), DeleteMonitor (lines 73-79). Protocol is restJson1, sigv4 name `networkmonitor` (lines 10-13). Handler evidence: `src/ops/create_monitor.rs` lines 4-6 carry `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 191 returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateMonitor"))`; all 12 files under `src/ops/` are the same generated stub, so no state is held anywhere and no tests assert any behaviour.

## Steps to Reproduce

1. Start the emulator with the networkmonitor service mounted.
2. aws networkmonitor create-monitor --monitor-name demo-monitor --aggregation-period 30 --tags team=net  =>  expect 200 with monitorArn arn:aws:networkmonitor:<region>:<account>:monitor/demo-monitor, monitorName demo-monitor, a MonitorState value and the tags echoed from stored state (actual: HTTP 501 NotImplemented).
3. aws networkmonitor create-probe --monitor-name demo-monitor --probe sourceArn=arn:aws:ec2:us-east-1:123456789012:subnet/subnet-0123456789abcdef0,destination=10.0.0.1,protocol=ICMP,packetSize=56  =>  expect 200 with a generated probeId matching ^probe-[a-z0-9A-Z-]{21,64}$ and a probeArn (actual: 501).
4. aws networkmonitor get-monitor --monitor-name demo-monitor  =>  expect 200 whose probes list contains the probeId from the previous step, plus createdAt/modifiedAt/aggregationPeriod 30 (actual: 501).
5. aws networkmonitor list-monitors --max-results 1  =>  expect demo-monitor in monitors; repeat with the returned nextToken to page, and confirm nextToken is absent on the last page (actual: 501).
6. aws networkmonitor delete-probe --monitor-name demo-monitor --probe-id <probeId>  then get-monitor  =>  expect the probes list to no longer contain that probeId (actual: 501).
7. aws networkmonitor delete-monitor --monitor-name demo-monitor  then get-monitor --monitor-name demo-monitor  =>  expect ResourceNotFoundException (HTTP 404) and demo-monitor absent from list-monitors (actual: 501).
