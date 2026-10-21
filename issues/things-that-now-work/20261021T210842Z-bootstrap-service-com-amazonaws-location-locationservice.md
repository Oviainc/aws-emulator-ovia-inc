Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the location-locationservice emulator

_Filed: 2026-10-21T21:08:42Z_
_Operation: service-com-amazonaws-location-locationservice | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 64 operations in `services/service-com-amazonaws-location-locationservice/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented` while holding zero state. No client workflow works at all.

This issue scopes the FIRST buildable slice only: the tracker + device-position happy path, which is Amazon Location Service's core tracking loop (protocol `restJson1`, sigv4 signing name `geo`). Implement these 6 operations statefully, in this order:

1. **CreateTracker** — `POST /tracking/v0/trackers`. Input: `TrackerName` (required, 1..100, `^[-._\w]+$`), `Description`, `PositionFiltering` (AccuracyBased|DistanceBased|TimeBased), `KmsKeyId`, `EventBridgeEnabled`, `Tags`, `PricingPlan`. Must persist a new Tracker and return `TrackerName`, a generated `TrackerArn` (`arn:aws:geo:<region>:<account>:tracker/<TrackerName>`) and `CreateTime`. Creating a name that already exists must return `ConflictException` (409).
2. **DescribeTracker** — `GET /tracking/v0/trackers/{TrackerName}`. Must read the stored tracker and return `TrackerName`, `TrackerArn`, `Description` (empty string when unset), `CreateTime`, `UpdateTime`, `PositionFiltering`, `Tags`, `KmsKeyId`, `EventBridgeEnabled` — exactly the values CreateTracker stored, not echoed input. Unknown name → `ResourceNotFoundException` (404).
3. **ListTrackers** — `POST /tracking/v0/list-trackers`. Must return one `ListTrackersResponseEntry{TrackerName, Description, CreateTime, UpdateTime}` per stored tracker, honouring `MaxResults`/`NextToken` (paginated: inputToken=NextToken, outputToken=NextToken, items=Entries). A deleted tracker must disappear from the list.
4. **BatchUpdateDevicePosition** — `POST /tracking/v0/trackers/{TrackerName}/positions`. Input `Updates`: list of `DevicePositionUpdate{DeviceId (required), SampleTime (required, ISO-8601), Position (required, exactly 2 doubles [lon, lat]), Accuracy, PositionProperties (<=4 entries)}`. Must append each update to that device's position history under the named tracker, stamping a server-side `ReceivedTime`. Per-entry failures (unknown device id shape, bad position arity) go into the `Errors` list with `DeviceId`, `SampleTime` and an inner `Error`; unknown tracker → `ResourceNotFoundException` (404). The list must never be fabricated — an all-valid batch returns `Errors: []`.
5. **GetDevicePosition** — `GET /tracking/v0/trackers/{TrackerName}/devices/{DeviceId}/positions/latest`. Must return the LATEST stored position for that device (highest `SampleTime`): `DeviceId`, `SampleTime`, `ReceivedTime`, `Position`, `Accuracy`, `PositionProperties`. Device with no stored position, or unknown tracker → `ResourceNotFoundException` (404).
6. **DeleteTracker** — `DELETE /tracking/v0/trackers/{TrackerName}`. Must remove the tracker and its device histories; empty body on success; unknown name → `ResourceNotFoundException` (404). A subsequent DescribeTracker/GetDevicePosition must 404 and ListTrackers must not list it.

Every one of these must compute its response from persisted state held in the shared engine (`crate::state::service_state(&*self.engine)`) — no canned bodies, no echoing the request back, no generating an ARN/ID without storing it. A create followed by a describe must return what was created; a position written by BatchUpdateDevicePosition must be readable by GetDevicePosition in a later request.

## State Model

Stored in `ComAmazonawsLocationLocationserviceState` (src/state.rs), under one lock:

- `trackers: HashMap<String /*TrackerName*/, Tracker>`
- `Tracker { name: String, arn: String, description: String, pricing_plan: Option<String>, position_filtering: String /*default TimeBased*/, kms_key_id: Option<String>, event_bridge_enabled: bool, kms_key_enable_geospatial_queries: bool, tags: BTreeMap<String,String>, create_time: SystemTime, update_time: SystemTime, consumer_arns: BTreeSet<String>, devices: HashMap<String /*DeviceId*/, Vec<StoredPosition>> }`
- `StoredPosition { device_id: String, sample_time: SystemTime, received_time: SystemTime, position: [f64; 2], accuracy: Option<f64 /*Horizontal*/>, position_properties: BTreeMap<String,String> }` — history kept sorted ascending by `sample_time`, so "latest" is the last element.

The `consumer_arns` set and a `next_token` opaque-cursor helper (index-or-key based, validated and rejected with `ValidationException` when malformed) are part of the state so the paginated list ops and the consumer ops can be layered on later without a state rewrite.

The other 58 operations (maps, place indexes, route calculators, geofence collections, API keys, jobs, tagging, Verify/ForecastGeofenceEvents, …) stay at 501 and will be filed as individual gap issues — that is expected and correct.

## Evidence

`services/service-com-amazonaws-location-locationservice/OPERATIONS.md` lists all 64 operations with the exact routes, input/output members and errors cited above (CreateTracker lines 411-417, DescribeTracker 419-425, DeleteTracker 435-441, ListTrackers 443-449, BatchUpdateDevicePosition 473-478, GetDevicePosition 487-493; protocol `restJson1`, sigv4 name `geo` at lines 10-13). `smithy_query shape=DevicePositionUpdate,DevicePosition,ListTrackersResponseEntry` confirms the member names/requirements of the batch update and position payloads (DeviceId/SampleTime/Position required, Position length exactly 2, PositionProperties max 4, ReceivedTime required on reads). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-location-locationservice/src/ops/` matches every handler file (including create_tracker.rs, describe_tracker.rs, list_trackers.rs, batch_update_device_position.rs, get_device_position.rs, delete_tracker.rs), so no operation has any behavior today. No tests exist beyond the generated `tests/state_roundtrip.rs` state-API example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `location` client at it.
2. `aws location create-tracker --tracker-name demo-tracker --position-filtering TimeBased --description "demo"` → expect 200 with TrackerName=demo-tracker, a geo tracker ARN and CreateTime; today: HTTP 501 NotImplemented.
3. `aws location describe-tracker --tracker-name demo-tracker` → expect the stored description, PositionFiltering, CreateTime/UpdateTime back.
4. `aws location list-trackers` → expect one entry for demo-tracker.
5. `aws location batch-update-device-position --tracker-name demo-tracker --updates DeviceId=device-1,SampleTime=2024-01-01T00:00:00Z,Position=-122.4,37.8` → expect 200 with an empty Errors list.
6. `aws location get-device-position --tracker-name demo-tracker --device-id device-1` → expect Position [-122.4, 37.8], the SampleTime sent, and a server-assigned ReceivedTime.
7. `aws location get-device-position --tracker-name demo-tracker --device-id unknown-device` → expect ResourceNotFoundException (404).
8. `aws location delete-tracker --tracker-name demo-tracker`, then re-run describe-tracker (expect 404) and list-trackers (expect no entries).
