Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: location.batch-update-device-position-partial-failure-only-valid-persists
Issue-Shape-Id: com.amazonaws.location#LocationService
Issue-Key: location.batch-update-device-position-partial-failure-only-valid-persists|run#12|AssertionError|assert errors[0]["Error"]["Message"] == "longitude must be between -180 and 180 degrees", (
Issue-Commit: 81e1f22df7dda30cfdd920fff09643b1094d9644

# BatchUpdateDevicePosition per-entry ValidationError message is emulator-worded: it prefixes the member name and appends the offending value

> _Note: this issue was identified against the code base at commit `81e1f22df7dda30cfdd920fff09643b1094d9644`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchUpdateDevicePosition

## Description
One net-new root cause in the location tracking data plane. The @timestampFormat date-time input defect that previously refused every BatchUpdateDevicePosition (open issue 20261030T001704Z-location-tracking-data-plane-refuses-every-batchupdatedeviceposition-the.md) no longer fires on this check: the batch now answers HTTP 200 and reports the bad entry inside Errors, with the right count, the right DeviceId and the right Error.Code. What diverges now is the per-entry Error.Message text, which that issue does not name as its cause and which is a distinct defect in this service's coordinate-domain validator.

Cause 1 of 1: the BatchItemError.Message for a position outside the WGS 84 longitude/latitude domain is emulator-authored wording rather than AWS's sentence. Failing check: location.batch-update-device-position-partial-failure-only-valid-persists.

AWS: Smithy (services/service-com-amazonaws-location-locationservice/smithy-model/com-amazonaws-location-locationservice.json): BatchUpdateDevicePosition POST /tracking/v0/trackers/{TrackerName}/positions, success status 200; output Errors (BatchUpdateDevicePositionErrorList, required). Each BatchUpdateDevicePositionError carries DeviceId (required), SampleTime (required, @timestampFormat date-time, @sensitive) and Error (BatchItemError, required) whose members are Code (BatchItemErrorCode — "ValidationError" is its "Input fails to satisfy the constraints specified by the service" member) and Message ("A message with the reason for the batch request error"). DevicePositionUpdate$Position targets Position, a @sensitive list of Double with @length 2..2 and NO @range on its members, so the WGS 84 coordinate domain is a service-side check whose refusal text the model does not state — recorded behavior only: for a longitude outside [-180, 180] AWS reports, inside Errors, Error.Code "ValidationError" and Error.Message "longitude must be between -180 and 180 degrees". That whole sentence is FIXED text: the only parts that could vary are the two bound numbers, which are the WGS 84 domain constants (±180 for longitude; the symmetric latitude sentence uses ±90). The offending value is NOT part of the message, and neither is the member name — note Position is @sensitive, so echoing the submitted coordinate back puts sensitive request data into an error string.

Emulator: the same entry yields Error.Message "Position longitude must be between -180 and 180 degrees; got 200". Two deviations from AWS's sentence: the member name "Position " is prepended, and "; got <longitude>" is appended with the value taken from the request. The latitude branch has the identical shape ("Position latitude must be between -90 and 90 degrees; got <latitude>"), so the defect is both branches of one validator, not one string. Error.Code ("ValidationError"), the Errors list length (1) and the DeviceId named in the entry all match AWS; only the message text diverges.

Why: a client that classifies a batch rejection by its message — the only free-text explanation the batch contract offers for a per-entry failure, since Code is the single coarse value "ValidationError" — does not recognise the emulator's sentence, so retry/skip logic keyed on it falls through to an unknown-error path that AWS would never take. Because the message interpolates the submitted coordinate, it is also not a stable string to match on at all, and it leaks a value from the @sensitive Position member into an error field AWS keeps free of it. The assertion failing here is also the gate for the rest of the documented partial-failure contract in this check: the two assertions AFTER it — that the valid entry is still committed and readable through GetDevicePosition with Position [-122.33, 47.60], and that the rejected entry was never stored (GetDevicePosition answering ResourceNotFoundException) — were never reached, so the partial-commit half of this behaviour is still unverified and must be confirmed by the same re-replay.

Fix: the coordinate-domain validator in this service's shared state module — the two Err(format!(...)) arms of crate::state::wgs84_position — must state AWS's fixed sentence with no member-name prefix and no echoed value, for the longitude and the latitude branch alike. The BatchUpdateDevicePosition handler already routes that string correctly into BatchItemError{Code: "ValidationError", Message} per entry and already keeps the valid entries, so no change is needed in the handler's batching or persistence path. File and line citations are under Traces.

## Evidence
Check location.batch-update-device-position-partial-failure-only-valid-persists, verdict mismatch:

  assert errors[0]["Error"]["Message"] == "longitude must be between -180 and 180 degrees"
  AssertionError: unexpected error message 'Position longitude must be between -180 and 180 degrees; got 200'

Request: CreateTracker, then BatchUpdateDevicePosition with Updates = [
  {DeviceId: "sim2real-good", SampleTime: <now, UTC>, Position: [-122.33, 47.60]},
  {DeviceId: "sim2real-bad",  SampleTime: <now, UTC>, Position: [200.0, 47.60]}]

Expected (AWS): HTTP 200; Errors has exactly one entry; entry.DeviceId == "sim2real-bad"; entry.Error.Code == "ValidationError"; entry.Error.Message == "longitude must be between -180 and 180 degrees".
Actual (emulator): HTTP 200; Errors has exactly one entry; entry.DeviceId == "sim2real-bad"; entry.Error.Code == "ValidationError"; entry.Error.Message == "Position longitude must be between -180 and 180 degrees; got 200".

Message decomposition of the divergence:
- AWS fixed text: "longitude must be between -180 and 180 degrees" (the two numbers are the WGS 84 longitude bounds, not request values; the latitude sentence is the same template with -90/90).
- Emulator adds the fixed prefix "Position " (the model member name) and the suffix "; got " followed by a VALUE read from the request's Position[0]. "200" in the quoted string above is this run's submitted longitude filling that interpolation, not part of any template.

Source of the emulator text: services/service-com-amazonaws-location-locationservice/src/state.rs, wgs84_position — the longitude arm at lines 373-378 ("Position longitude must be between {} and {MAX_LONGITUDE} degrees; got {longitude}") and the latitude arm at lines 379-384. It is consumed per entry by services/service-com-amazonaws-location-locationservice/src/ops/batch_update_device_position.rs lines 220-242, which builds BatchItemError{code: crate::state::BATCH_VALIDATION_ERROR ("ValidationError"), message: Some(why)}.

Not reached because the assertion above fails first (still unverified after the fix): GetDevicePosition for "sim2real-good" returning Position [-122.33, 47.60] within 30s, and GetDevicePosition for "sim2real-bad" answering ResourceNotFoundException because the rejected entry was never stored.

Related open issues that do NOT cover this: 20261030T001704Z-location-tracking-data-plane-refuses-every-batchupdatedeviceposition-the.md (input date-time deserialization; the request is no longer refused on this check and that issue never names the per-entry message wording) and 20261029T234705Z-location-refusal-bodies-are-emulator-authored-and-omit.md (request-level ValidationException Reason/FieldList, not the BatchItemError.Message inside a 200 body).

## Steps to Reproduce
1. CreateTracker with any name satisfying ^[-._\w]+$ (POST /tracking/v0/trackers).
2. Call BatchUpdateDevicePosition on that tracker with two updates: one in-domain position (e.g. [-122.33, 47.60]) and one whose longitude is outside [-180, 180] (e.g. [200.0, 47.60]), each with a required ISO 8601 SampleTime.
3. Read the single entry in the 200 response's Errors list. Observe Error.Message "Position longitude must be between -180 and 180 degrees; got 200" where AWS reports the fixed sentence "longitude must be between -180 and 180 degrees" with no member-name prefix and no echoed coordinate.
4. Repeat with an out-of-domain latitude (e.g. [-122.33, 120.0]) to see the same deviation on the latitude branch: "Position latitude must be between -90 and 90 degrees; got 120" instead of "latitude must be between -90 and 90 degrees".
5. Re-replay location.batch-update-device-position-partial-failure-only-valid-persists; after the message is corrected the check continues into its partial-commit assertions (valid entry readable through GetDevicePosition, rejected entry ResourceNotFoundException), which must also pass.
