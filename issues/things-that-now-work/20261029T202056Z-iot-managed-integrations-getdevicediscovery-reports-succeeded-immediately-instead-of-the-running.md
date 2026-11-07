Issue-Source: trace-sweep-divergence
Issue-Trace: iot-managed-integrations-device-discovery-listing-filters
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-managed-integrations-device-discovery-listing-filters
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations|GetDeviceDiscovery|2xx->2xx|$.ControllerId
Issue-Key: iot-managed-integrations|GetDeviceDiscovery|2xx->2xx|$.Id
Issue-Key: iot-managed-integrations|GetDeviceDiscovery|2xx->2xx|$.Status
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# GetDeviceDiscovery reports SUCCEEDED immediately instead of the RUNNING state AWS returns after StartDeviceDiscovery

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDeviceDiscovery

## Description
A device-discovery job created by StartDeviceDiscovery is served by GetDeviceDiscovery with a terminal Status of SUCCEEDED the instant it is created, so the RUNNING phase of the job never exists in the emulator.

Cause 1 of 1: GetDeviceDiscovery returns Status=SUCCEEDED for a freshly started discovery (fixture iot-managed-integrations-device-discovery-listing-filters, step task#3).

AWS: StartDeviceDiscovery (POST /device-discoveries, 201) returns only Id and StartedAt; the job it creates is asynchronous. GetDeviceDiscovery (GET /device-discoveries/{Identifier}, 200) has a required Status member targeting the enum DeviceDiscoveryStatus with the value set [RUNNING, SUCCEEDED, FAILED, TIMED_OUT], and a FinishedAt member that is NOT required, i.e. the job is modelled as reaching a terminal state only later. Smithy: GetDeviceDiscovery.Status -> DeviceDiscoveryStatus (required, enum RUNNING|SUCCEEDED|FAILED|TIMED_OUT); GetDeviceDiscovery.FinishedAt -> DiscoveryFinishedAt (optional); StartDeviceDiscovery output = {Id, StartedAt} only. The recording shows that a GetDeviceDiscovery issued immediately after a successful StartDeviceDiscovery in the same fixture answers 200 with Status 'RUNNING' — the job is still in flight at that point.

Emulator: the same immediate GetDeviceDiscovery answers 200 with Status 'SUCCEEDED'. The status is therefore not a tracked lifecycle value at all: the discovery is written to state already terminal, so RUNNING is never observable for any discovery, however soon it is read.

Why: a client that polls GetDeviceDiscovery until Status leaves RUNNING before reading the discovered endpoint devices exits its poll loop on the very first call and proceeds against a discovery that, on AWS, has not produced results yet — so the emulator cannot be used to exercise or test any discovery-completion wait, timeout or retry path. The same value feeds ListDeviceDiscoveries status filtering, so a filter on RUNNING matches nothing in the emulator while on AWS it matches a just-started job.

Fix: the device-discovery state in the iot-managed-integrations crate plus the StartDeviceDiscovery / GetDeviceDiscovery handlers: a discovery created by StartDeviceDiscovery must be recorded with Status=RUNNING and no FinishedAt, and GetDeviceDiscovery must serve that stored status rather than a constant terminal one, only reporting a terminal status (and then setting FinishedAt) once the emulator's own completion rule for the job has fired.

Traces: iot-managed-integrations-device-discovery-listing-filters#task#3 (expected Status 'RUNNING', got 'SUCCEEDED'). The Id/ControllerId entries reported alongside it in that same cluster are per-run generated identifiers and are not part of this defect.

## Evidence
Replay of fixture iot-managed-integrations-device-discovery-listing-filters, step task#3, operation GetDeviceDiscovery: expected status 200, actual status 200 (status matches). Body divergence reported by the replay: "$.Status: value_mismatch (expected 'RUNNING', got 'SUCCEEDED')". Expected: Status is the non-terminal enum value RUNNING for a discovery started earlier in the same fixture. Actual: Status is the terminal enum value SUCCEEDED. Both values are members of the modelled enum DeviceDiscoveryStatus [RUNNING, SUCCEEDED, FAILED, TIMED_OUT], so this is a lifecycle-state divergence and not a serialization or casing difference. The $.ControllerId and $.Id entries in the same cluster compare generated identifiers and are environment-specific, not requirements.

## Steps to Reproduce
1. Replay fixture iot-managed-integrations-device-discovery-listing-filters against the emulator.
2. The fixture starts a device discovery with StartDeviceDiscovery (POST /device-discoveries), which answers 201 with an Id.
3. At step task#3 it calls GetDeviceDiscovery (GET /device-discoveries/{Identifier}) with that Id.
4. Read $.Status in the 200 response body: the emulator returns 'SUCCEEDED' where the recording returns 'RUNNING' for the same immediately-following read.
