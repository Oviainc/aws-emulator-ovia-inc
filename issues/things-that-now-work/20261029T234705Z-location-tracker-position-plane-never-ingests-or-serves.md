Issue-Source: trace-sweep-divergence
Issue-Trace: location-device-position-roundtrip-and-not-found
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: location-device-position-roundtrip-and-not-found
Issue-Shape-Id: com.amazonaws.location#LocationService
Issue-Key: location|BatchUpdateDevicePosition|2xx->4xx|$.Errors
Issue-Key: location|BatchUpdateDevicePosition|2xx->4xx|status
Issue-Key: location|GetDevicePosition|2xx->4xx|$.DeviceId
Issue-Key: location|GetDevicePosition|2xx->4xx|$.Position
Issue-Key: location|GetDevicePosition|2xx->4xx|$.ReceivedTime
Issue-Key: location|GetDevicePosition|2xx->4xx|$.SampleTime
Issue-Key: location|GetDevicePosition|2xx->4xx|status
Issue-Commit: c1b8e21acf710dd592fea1840f63adefca5c3a99

# Location tracker position plane never ingests or serves device positions

> _Note: this issue was identified against the code base at commit `c1b8e21acf710dd592fea1840f63adefca5c3a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchUpdateDevicePosition

## Description
Two causes, one shared fault: the tracker position store is never written, so the write side refuses the batch and the read side then reports the device as unknown. A client cannot complete the documented update-then-read round trip on a tracker.

Cause 1 of 2: BatchUpdateDevicePosition refuses a well-formed batch instead of accepting it.
AWS: Smithy: BatchUpdateDevicePosition POST /tracking/v0/trackers/{TrackerName}/positions; input TrackerName (@httpLabel, required, @length 1..100, @pattern ^[-._\w]+$) and Updates (DevicePositionUpdateList, required, @length 1..10); output Errors (BatchUpdateDevicePositionErrorList, required). A batch whose tracker exists and whose updates satisfy the modelled constraints answers HTTP 200 with Errors present and empty — per-update failures are reported as entries inside Errors, never as a request-level refusal. Modelled refusals are ValidationException 400, ResourceNotFoundException 404, AccessDeniedException 403, ThrottlingException 429, InternalServerException 500.
Emulator: answers HTTP 400 and the body carries no Errors member at all (expected [], got None).
Why: a client that batches positions for a tracker gets a hard client error for a request AWS accepts, so no position is ever recorded and the partial-failure contract (inspect Errors, retry only the failed entries) is unreachable.
Fix: the BatchUpdateDevicePosition handler and its input validation in the location service crate — accept the modelled batch, persist each update against the tracker, and always serialize the required Errors list.
Traces: location-device-position-roundtrip-and-not-found#task#2.

Cause 2 of 2: GetDevicePosition reports 404 for a device whose position was just reported.
AWS: Smithy: GetDevicePosition GET /tracking/v0/trackers/{TrackerName}/devices/{DeviceId}/positions/latest; output DeviceId, SampleTime (required), ReceivedTime (required), Position (required, @length 2..2), optional Accuracy and PositionProperties. After a position update for that device the latest-position read answers HTTP 200 carrying the device id, the sample time the caller supplied, a service-assigned received time and the two-element [longitude, latitude] position.
Emulator: answers HTTP 404 and the body carries none of DeviceId, SampleTime, ReceivedTime or Position.
Why: the latest-position read — the only way to observe a tracked device — reports every device as unknown, so tracking state is invisible to clients even when the write path is fixed.
Fix: the GetDevicePosition handler and the tracker position state it reads in the location service crate; the device's latest position must be keyed by tracker name + device id and returned with the modelled required members.
Traces: location-device-position-roundtrip-and-not-found#task#3.

## Evidence
Cause 1 — fixture location-device-position-roundtrip-and-not-found step task#2, BatchUpdateDevicePosition: expected status 200, got 400; $.Errors expected [] got None (member absent).
Cause 2 — same fixture step task#3, GetDevicePosition: expected status 200, got 404; $.DeviceId expected the reported device id, got None; $.SampleTime expected the ISO 8601 sample time supplied on the update, got None; $.ReceivedTime expected a service-assigned timestamp (shape asserted only), got None; $.Position expected the two-element [longitude, latitude] list supplied on the update, got None.

## Steps to Reproduce
1. Replay fixture location-device-position-roundtrip-and-not-found against the emulator.
2. Step task#2 — POST /tracking/v0/trackers/{TrackerName}/positions (BatchUpdateDevicePosition) with one update for a device on an existing tracker: observe HTTP 400 with no Errors member where AWS answers 200 with "Errors": [].
3. Step task#3 — GET /tracking/v0/trackers/{TrackerName}/devices/{DeviceId}/positions/latest (GetDevicePosition) for that same device: observe HTTP 404 where AWS answers 200 with DeviceId, SampleTime, ReceivedTime and Position.
