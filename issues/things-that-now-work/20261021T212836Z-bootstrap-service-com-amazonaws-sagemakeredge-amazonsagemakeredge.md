Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the Amazon SageMaker Edge Manager (sagemakeredge) emulator

_Filed: 2026-10-21T21:28:36Z_
_Operation: service-com-amazonaws-sagemakeredge-amazonsagemakeredge | Class: stub_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of this service's three operations is an unmodified scaffold: `src/ops/get_device_registration.rs`, `src/ops/send_heartbeat.rs` and `src/ops/get_deployments.rs` all carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and `src/state.rs` holds `// TODO: add service-specific state fields` — i.e. zero state. The service surface is tiny, so the bootstrap slice is the complete edge-agent loop; all three operations are in scope.

Wire protocol: `restJson1`, sigv4 signing name `sagemaker`, endpoint prefix `edge.sagemaker` (see README/OPERATIONS.md).

### Happy path to implement (in order)

1. **GetDeviceRegistration** — `POST /GetDeviceRegistration`. Input: `DeviceName` (required, 1..63), `DeviceFleetName` (required, 1..63). Output: `DeviceRegistration` (string 1..1000 — the registration/authorization document the agent caches), `CacheTTL` (string 1..1000). Must look the device up in state by `(DeviceFleetName, DeviceName)` and return the registration document and TTL recorded for THAT device — not a constant string. First contact from an unknown device registers it in state (so a subsequent call returns the same document and the heartbeat path has somewhere to land).
2. **SendHeartbeat** — `POST /SendHeartbeat`. Input: `DeviceName`, `DeviceFleetName`, `AgentVersion` (all required), plus optional `AgentMetrics` (list<EdgeMetric>{Dimension, MetricName, Value, Timestamp}), `Models` (list<Model>{ModelName, ModelVersion, LatestSampleTime, LatestInference, ModelMetrics}) and `DeploymentResult` (DeploymentResult{DeploymentName, DeploymentStatus, DeploymentStatusMessage, DeploymentStartTime, DeploymentEndTime, DeploymentModels}). Output has no members, so the ONLY observable effect is state: the handler must persist `AgentVersion`, a `last_heartbeat_at` timestamp, the reported `Models` and `AgentMetrics` on the device record, and — when `DeploymentResult` is present — record that result against the matching pending deployment for the device so it is no longer handed back by GetDeployments. A heartbeat that drops its payload is a hollow 200 and is not acceptable.
3. **GetDeployments** — `POST /GetDeployments`. Input: `DeviceName`, `DeviceFleetName` (both required). Output: `Deployments` (list<EdgeDeployment>{DeploymentName, Type, FailureHandlingPolicy, Definitions:list<Definition>{ModelHandle, S3Url, Checksum, State}}). Must return exactly the deployments currently outstanding for that device in that fleet, read from state: deployments acknowledged via a prior SendHeartbeat `DeploymentResult` must NOT reappear, and an unknown/empty device must get an empty list (not a canned deployment).

Because the real control-plane that creates edge deployments lives in the `sagemaker` service (CreateEdgeDeploymentPlan / device-fleet APIs), the emulator needs a seam for seeding deployments into a fleet so GetDeployments can return something non-empty: store deployments per fleet in this crate's state and let them be seeded (fixture/test helper or cross-crate state write). GetDeployments itself must never fabricate them.

All three handlers must compute their response from the persisted state reached via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoed inputs, no random IDs without storage. Delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line from each file as it is implemented. `InternalServiceException` (HTTP 400) is the only modeled error; refuse malformed-but-unmodeled input with `Scaffolded::validation(..)` (400) rather than 501.

## State Model

```rust
pub struct ComAmazonawsSagemakeredgeAmazonsagemakeredgeState {
    /// fleet name -> fleet record
    fleets: HashMap<String, Fleet>,
}

struct Fleet {
    name: String,
    /// device name -> device record (devices are per-fleet)
    devices: HashMap<String, Device>,
    /// deployments seeded for this fleet, fanned out to its devices
    deployments: Vec<Deployment>,
}

struct Device {
    name: String,
    fleet_name: String,
    /// returned verbatim by GetDeviceRegistration
    registration: String,
    cache_ttl: String,
    /// last values reported by SendHeartbeat
    agent_version: Option<String>,
    last_heartbeat_at: Option<SystemTime>,
    models: Vec<ReportedModel>,       // ModelName, ModelVersion, LatestSampleTime, LatestInference, ModelMetrics
    agent_metrics: Vec<EdgeMetric>,   // Dimension, MetricName, Value, Timestamp
    /// deployment name -> result reported by the agent (acknowledgement)
    deployment_results: HashMap<String, DeploymentResultRecord>,
}

struct Deployment {
    deployment_name: String,          // unique within the fleet
    deployment_type: Option<String>,  // DeploymentType enum
    failure_handling_policy: Option<String>,
    definitions: Vec<Definition>,     // ModelHandle, S3Url, Checksum{Type,Sum}, State
}

struct DeploymentResultRecord {
    status: Option<String>,
    status_message: Option<String>,
    start_time: Option<SystemTime>,
    end_time: Option<SystemTime>,
    models: Vec<ReportedModel>,
}
```

GetDeployments(device) = fleet.deployments filtered to those whose `deployment_name` has no entry in `device.deployment_results`.

Done when a realistic edge-agent script (below) runs end-to-end with correct, stateful bodies — not merely 2xx. The service has no other operations, so this slice is the whole first implementation.

## Evidence

OPERATIONS.md for this crate lists exactly 3 operations with their routes, required members and the single `InternalServiceException (HTTP 400)` error: GetDeployments (`POST /GetDeployments`, out `Deployments: list<EdgeDeployment>`), GetDeviceRegistration (`POST /GetDeviceRegistration`, out `DeviceRegistration`, `CacheTTL`), SendHeartbeat (`POST /SendHeartbeat`, no output members). Field shapes (EdgeDeployment, Definition, Model, EdgeMetric, DeploymentResult) come from the Smithy model at services/service-com-amazonaws-sagemakeredge-amazonsagemakeredge/smithy-model/com-amazonaws-sagemakeredge-amazonsagemakeredge.json. All three handler files under src/ops/ are scaffolds: e.g. send_heartbeat.rs line 4 `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 180 `Err(crate::scaffolded::Scaffolded::not_implemented("SendHeartbeat"))`. src/state.rs line 8 is still `// TODO: add service-specific state fields`, so no operation could be stateful today. Well-established SageMaker Edge Manager agent behaviour grounds the loop: the agent fetches its registration document (cached for CacheTTL), heartbeats model/agent metrics plus the result of its last deployment, and polls GetDeployments for work.

## Steps to Reproduce

1. Start the emulator and point a sigv4 client at the `edge.sagemaker` endpoint (restJson1, signing name `sagemaker`).
2. POST /GetDeviceRegistration with {"DeviceName":"edge-device-1","DeviceFleetName":"fleet-a"} — expect HTTP 200 with a non-empty `DeviceRegistration` document and a `CacheTTL`; today it returns 501 NotImplemented.
3. Repeat the same GetDeviceRegistration call — expect the SAME `DeviceRegistration`/`CacheTTL` values (proving they are persisted, not generated per call).
4. Seed one deployment named "deploy-1" with a Definitions entry (ModelHandle "mnist", S3Url "s3://bucket/mnist.tar.gz", State) into fleet "fleet-a" via the state fixture seam.
5. POST /GetDeployments with {"DeviceName":"edge-device-1","DeviceFleetName":"fleet-a"} — expect 200 with Deployments[0].DeploymentName == "deploy-1" and its Definitions echoed from stored state.
6. POST /SendHeartbeat with DeviceName/DeviceFleetName, AgentVersion "1.20.0", Models [{ModelName:"mnist",ModelVersion:"1.0"}] and AgentMetrics [{MetricName:"cpu-usage",Value:12.5}] — expect 200 with an empty body, and the device record in state now carries the agent version, heartbeat timestamp, model list and metrics.
7. POST /SendHeartbeat again with DeploymentResult {DeploymentName:"deploy-1", DeploymentStatus:"SUCCESS", DeploymentStartTime, DeploymentEndTime, DeploymentModels:[{ModelName:"mnist",ModelVersion:"1.0"}]} — expect 200.
8. POST /GetDeployments again for the same device — expect 200 with an EMPTY `Deployments` list, because the deployment was acknowledged by the heartbeat.
9. POST /GetDeployments for an unknown device ("edge-device-9" / "fleet-a") — expect 200 with an empty Deployments list, never a fabricated deployment.

## Resolution

All three operations are implemented and answer from `src/state.rs` (fleets -> devices /
seeded deployments); the `SCAFFOLDED, NOT IMPLEMENTED` marker is gone from each handler.
Covered end-to-end by `tests/all/edge_agent_loop.rs`.

One deliberate departure from the plan above, with the consequence for the script in
"Steps to Reproduce": **GetDeviceRegistration does not register a device.** The model
documents that operation as "Use to check if a device is registered with SageMaker Edge
Manager" — it reports — and a reporting operation must not change what a later caller
sees. A device record is therefore created only where a write is sanctioned: by
`SendHeartbeat` (the agent reporting the device in) or by the `register_device` seam that
stands in for the `sagemaker` control plane's device-fleet APIs. Steps 2 and 3 answer 200
with a document reporting `Status: NOT_REGISTERED` for a device nothing has registered yet
(identical on every read, since nothing is generated per call), and **step 5 reports
"deploy-1" only once `edge-device-1` has been registered** — by the step-6 heartbeat, or by
seeding it through `register_device` next to the step-4 `seed_deployment`. Steps 6 to 9 hold
as written. Both registration paths are exercised in the test suite, which asserts that how
a device record was created does not change what GetDeployments reports.

Request-keyed state is bounded (`MAX_FLEETS`, `MAX_DEVICES_PER_FLEET`,
`MAX_DEPLOYMENTS_PER_FLEET`, `MAX_DEPLOYMENT_RESULTS_PER_DEVICE`): a heartbeat that would
grow the store past a bound is refused with 400 rather than growing it.
