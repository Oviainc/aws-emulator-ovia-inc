Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: location.batch-update-device-position-partial-failure-only-valid-persists location.device-position-history-pagination-respects-max-results location.device-position-history-rejects-inverted-time-window location.device-position-history-time-window-filters-results
Issue-Shape-Id: com.amazonaws.location#LocationService
Issue-Key: location|BatchUpdateDevicePosition|ValidationException
Issue-Commit: 5be2d973ad1b703e2cd0a56e2c9e00ba2fe90954

# Location tracking data plane refuses every BatchUpdateDevicePosition: the @timestampFormat date-time SampleTime is deserialized as an epoch integer

> _Note: this issue was identified against the code base at commit `5be2d973ad1b703e2cd0a56e2c9e00ba2fe90954`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchUpdateDevicePosition

## Description
One cause, and it blocks the whole tracker position plane: the request deserializer does not honour the @timestampFormat date-time trait on the input timestamp members of the tracking operations, so it tries to read the ISO 8601 string every AWS SDK sends as an epoch integer and the request is refused before the (implemented) handler ever runs.

Cause 1 of 1: BatchUpdateDevicePosition refuses a well-formed batch with 400 ValidationException 'invalid timestamp: failed to parse int'.
AWS: Smithy: BatchUpdateDevicePosition POST /tracking/v0/trackers/{TrackerName}/positions, input TrackerName (@httpLabel, required, @length 1..100, @pattern ^[-._\w]+$) and Updates (DevicePositionUpdateList, required, @length 1..10); each DevicePositionUpdate has DeviceId (required), SampleTime (required, Timestamp, @timestampFormat date-time, @sensitive, documented 'Uses ISO 8601 format: YYYY-MM-DDThh:mm:ss.sssZ') and Position (required, @length 2..2). Because SampleTime carries @timestampFormat date-time, the JSON body member is an ISO 8601 date-time STRING, not the restJson1 default epoch-seconds number — botocore serializes it that way. A batch whose tracker exists and whose entries satisfy the modelled constraints answers HTTP 200 with the required Errors list (empty when every entry was accepted); a per-entry failure is reported INSIDE Errors as a BatchUpdateDevicePositionError {DeviceId, SampleTime, Error: BatchItemError{Code, Message}} with Code 'ValidationError', never as a request-level refusal. The same trait is on GetDevicePositionHistory's StartTimeInclusive and EndTimeExclusive, so those two query members are ISO 8601 strings as well. Modelled refusals are ValidationException 400, ResourceNotFoundException 404, AccessDeniedException 403, ThrottlingException 429, InternalServerException 500; ValidationException requires Message (jsonName message), Reason (jsonName reason) and FieldList (jsonName fieldList).
Emulator: every BatchUpdateDevicePosition call — including a single valid entry for an existing tracker — is refused with HTTP 400, error code 'ValidationException' and message 'invalid timestamp: failed to parse int'. 'invalid timestamp: failed to parse int' is entirely emulator-authored fixed text with no AWS counterpart; it names no member, and AWS has no error of this wording for this request class (AWS accepts the request, status 200). The refusal comes from the request boundary, not from the handler in services/service-com-amazonaws-location-locationservice/src/ops/batch_update_device_position.rs, which is implemented and would have stored the sample (see its wgs84_position/record_position path).
Why: the tracking data plane is completely unreachable for any AWS SDK client. No position can ever be written, so GetDevicePosition and GetDevicePositionHistory report every device as unknown; the documented partial-failure contract (inspect Errors, retry only the entries that failed) is unobservable; and the downstream behaviours these checks exist to verify — GetDevicePositionHistory's half-open StartTimeInclusive/EndTimeExclusive window filtering, its MaxResults/NextToken pagination in sample-time order, and its refusal of an inverted window with ValidationException "'EndTimeExclusive' must be after 'StartTimeInclusive'." — cannot be exercised at all because the positive-control write fails first. A client sees a hard 400 for a request real AWS accepts with 200.
Fix: the request deserialization/input schema for the location service's tracking operations — the generated INPUT_SCHEMA driven by smithy_macros::smithy_model! in services/service-com-amazonaws-location-locationservice/src/ops/batch_update_device_position.rs (and the equivalent for get_device_position_history.rs) must honour @timestampFormat date-time and parse the member as an ISO 8601 date-time string, falling back to epoch-seconds only where the model leaves the format defaulted. This is the INPUT counterpart of the already-open output-side defect in 20261029T234705Z-location-tracker-read-responses-serialize-timestamps-as-epoch.md (date-time members serialized as epoch numbers); the open issue 20261029T234705Z-location-tracker-position-plane-never-ingests-or-serves.md records the same 400 symptom from the wire corpus but diagnoses it as the handler/state never being written and does not name the timestamp deserialization, so neither covers this finding. Once the request parses, re-replay the four checks below to confirm the history window, pagination and inverted-window rule.
Traces: location.batch-update-device-position-partial-failure-only-valid-persists, location.device-position-history-pagination-respects-max-results, location.device-position-history-rejects-inverted-time-window, location.device-position-history-time-window-filters-results.

## Evidence
All four surviving location checks fail on their FIRST BatchUpdateDevicePosition call, with the identical boundary refusal:

botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the BatchUpdateDevicePosition operation: invalid timestamp: failed to parse int

Expected (AWS): HTTP 200 with body {"Errors": []} for a batch of one valid update against an existing tracker; for the mixed batch in location.batch-update-device-position-partial-failure-only-valid-persists, HTTP 200 with exactly one entry in Errors naming the out-of-domain device, Error.Code 'ValidationError' and Error.Message 'longitude must be between -180 and 180 degrees', while the valid entry is still committed and readable through GetDevicePosition.
Actual (emulator): HTTP 400, Error.Code 'ValidationException', Error.Message 'invalid timestamp: failed to parse int'. Message decomposition: the whole string is fixed emulator-authored text; it interpolates nothing from the request (no member name, no offending value) and corresponds to no AWS error wording for this request class.

The SampleTime values sent are ordinary timezone-aware datetimes (datetime.datetime.now(datetime.timezone.utc) and offsets from it), which botocore renders as ISO 8601 date-time strings because DevicePositionUpdate$SampleTime carries @timestampFormat date-time. 'failed to parse int' shows the emulator read that string with the restJson1 default epoch-seconds parser.

Blocked downstream assertions never reached:
- location.device-position-history-time-window-filters-results: StartTimeInclusive/EndTimeExclusive must select only the sample inside the half-open window.
- location.device-position-history-pagination-respects-max-results: MaxResults=1 must yield one page per sample via NextToken, in ascending sample-time order.
- location.device-position-history-rejects-inverted-time-window: an end not after the start must be refused with ValidationException and message "'EndTimeExclusive' must be after 'StartTimeInclusive'." (fixed text; the member names are the model's), leaving history unchanged.

## Steps to Reproduce
1. CreateTracker with any valid tracker name (e.g. POST /tracking/v0/trackers with TrackerName satisfying ^[-._\w]+$).
2. Call BatchUpdateDevicePosition with one update using any AWS SDK: Updates=[{DeviceId: 'sim2real-good', SampleTime: <timezone-aware datetime>, Position: [-122.33, 47.60]}] against that tracker, i.e. POST /tracking/v0/trackers/{TrackerName}/positions with SampleTime serialized as an ISO 8601 date-time string per @timestampFormat date-time.
3. Observe HTTP 400 ValidationException 'invalid timestamp: failed to parse int' where AWS answers HTTP 200 with {"Errors": []}.
4. Repeat with a mixed batch (one in-domain position, one longitude of 200.0): AWS still answers 200 and reports only the bad entry inside Errors with Code 'ValidationError'; the emulator refuses the whole request at the boundary.
5. Because step 2 never commits, GetDevicePosition and GetDevicePositionHistory for that device return no samples, so the history time-window, pagination and inverted-window checks cannot run.
6. Re-replay the four checks in trace_ids after fixing the date-time input parsing.
