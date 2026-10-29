Issue-Source: trace-sweep-divergence
Issue-Trace: location-tracker-crud-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: location-tracker-crud-lifecycle location-device-position-roundtrip-and-not-found location-map-data-plane-tile-sprites-glyphs-style
Issue-Shape-Id: com.amazonaws.location#LocationService
Issue-Key: location|CreateTracker|4xx->4xx|$.message
Issue-Key: location|DescribeTracker|4xx->4xx|$.message
Issue-Key: location|GetDevicePosition|4xx->4xx|$.message
Issue-Key: location|GetMapGlyphs|4xx->4xx|$.fieldList
Issue-Key: location|GetMapGlyphs|4xx->4xx|$.message
Issue-Key: location|GetMapGlyphs|4xx->4xx|$.reason
Issue-Commit: c1b8e21acf710dd592fea1840f63adefca5c3a99

# Location refusal bodies are emulator-authored and omit ValidationException Reason and FieldList

> _Note: this issue was identified against the code base at commit `c1b8e21acf710dd592fea1840f63adefca5c3a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTracker

## Description
Four causes, one shared fault: the location crate renders its own refusal text from a generic resource/constraint template instead of the service's wording, and in the validation case it also drops two members the model marks required. The HTTP statuses are right in all four cases; the bodies are not.

Cause 1 of 4: CreateTracker duplicate-name conflict message.
AWS: Smithy: CreateTracker POST /tracking/v0/trackers, error ConflictException (httpError 409) with one required member Message (jsonName "message"). Re-creating an existing tracker answers 409 with message "Tracker already exists: <TrackerName>." — fixed text "Tracker already exists: " plus the TrackerName taken from the request plus a terminating period.
Emulator: 409 with message 'Resource of type "Tracker" with name "<TrackerName>" already exists.'
Why: the status matches, so a client branches correctly, but any client or test that matches the conflict wording (or surfaces it to an operator) sees text the service never emits.
Fix: the CreateTracker handler's conflict path in the location service crate — raise ConflictException with the service's own sentence rather than the crate's generic resource template.
Traces: location-tracker-crud-lifecycle#task#4.

Cause 2 of 4: DescribeTracker not-found message.
AWS: Smithy: DescribeTracker GET /tracking/v0/trackers/{TrackerName}, error ResourceNotFoundException (httpError 404), required Message. A tracker that does not exist (here, after it was deleted) answers 404 with message "Tracker not found: <TrackerName>." — fixed text "Tracker not found: " plus the TrackerName from the request path plus a period.
Emulator: 404 with message 'Resource of type "Tracker" with name "<TrackerName>" cannot be found.'
Why: same generic template; the resource-type-and-name phrasing is not the service's and does not match client-side wording assertions.
Fix: the DescribeTracker handler's not-found path in the location service crate.
Traces: location-tracker-crud-lifecycle#task#6.

Cause 3 of 4: GetDevicePosition not-found message.
AWS: Smithy: GetDevicePosition, error ResourceNotFoundException (httpError 404), required Message. A tracker read for a device that has reported no position answers 404 with message "records not found for given deviceId" — entirely fixed text, with no device id, tracker name or other request value interpolated into it.
Emulator: 404 with message 'Resource of type "DevicePosition" with name "<DeviceId>" cannot be found.', i.e. the generic template plus the device id the service deliberately does not echo.
Why: this refusal is about absent position records, not an absent resource named DevicePosition; the emulator both re-words it and leaks the requested device id into text AWS keeps constant.
Fix: the GetDevicePosition handler's not-found path in the location service crate.
Traces: location-device-position-roundtrip-and-not-found#task#6.

Cause 4 of 4: GetMapGlyphs FontUnicodeRange validation refusal uses the generic Smithy constraint text and omits the required Reason and FieldList.
AWS: Smithy: GetMapGlyphs GET /maps/v0/maps/{MapName}/glyphs/{FontStack}/{FontUnicodeRange}; FontUnicodeRange is @httpLabel, required, @pattern ^[0-9]+-[0-9]+\.pbf$. Error ValidationException (httpError 400) has THREE required members — Message (jsonName "message"), Reason (jsonName "reason", enum ValidationExceptionReason) and FieldList (jsonName "fieldList", list of {name, message}). A FontUnicodeRange that does not match the pattern answers 400 with message "'FontUnicodeRange' must match regex ^[0-9]+-[0-9]+\\.pbf$" (fixed text "' ... ' must match regex " with the member name as modelled and the pattern taken from the model), reason "FieldValidationFailed", and fieldList carrying one entry whose name is the member ("FontUnicodeRange") and whose message repeats that same sentence.
Emulator: 400 with the generic constraint-validator sentence "1 validation error detected: Value '<value>' at 'fontUnicodeRange' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]+-[0-9]+\\.pbf$", and with reason and fieldList absent entirely.
Why: two members the model marks required are missing, so a strict SDK deserialization of ValidationException fails or yields nulls where the enum reason and per-field list are mandatory; and a client that routes on reason == FieldValidationFailed or reads fieldList[].name to highlight the offending input has nothing to read. The message is also lower-cased to the wire member spelling and wrapped in the generic Smithy preamble.
Fix: the location crate's shared validation/serializer path for ValidationException — emit the service's per-member sentence and always populate the required Reason and FieldList members; the GetMapGlyphs handler must route its FontUnicodeRange pattern check through it.
Traces: location-map-data-plane-tile-sprites-glyphs-style#task#6.

## Evidence
Cause 1 — location-tracker-crud-lifecycle#task#4 (CreateTracker, 409 both sides): $.message expected 'Tracker already exists: <TrackerName>.', got 'Resource of type "Tracker" with name "<TrackerName>" already exists.'
Cause 2 — location-tracker-crud-lifecycle#task#6 (DescribeTracker, 404 both sides): $.message expected 'Tracker not found: <TrackerName>.', got 'Resource of type "Tracker" with name "<TrackerName>" cannot be found.'
Cause 3 — location-device-position-roundtrip-and-not-found#task#6 (GetDevicePosition, 404 both sides): $.message expected 'records not found for given deviceId', got 'Resource of type "DevicePosition" with name "<DeviceId>" cannot be found.'
Cause 4 — location-map-data-plane-tile-sprites-glyphs-style#task#6 (GetMapGlyphs, 400 both sides): $.message expected "'FontUnicodeRange' must match regex ^[0-9]+-[0-9]+\\.pbf$", got "1 validation error detected: Value '<value>' at 'fontUnicodeRange' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]+-[0-9]+\\.pbf$"; $.reason expected 'FieldValidationFailed', got None; $.fieldList expected [{name: 'FontUnicodeRange', message: "'FontUnicodeRange' must match regex ^[0-9]+-[0-9]+\\.pbf$"}], got None.

## Steps to Reproduce
1. Replay fixtures location-tracker-crud-lifecycle, location-device-position-roundtrip-and-not-found and location-map-data-plane-tile-sprites-glyphs-style against the emulator.
2. CreateTracker with a TrackerName that already exists (location-tracker-crud-lifecycle task#4): compare the 409 body's message with 'Tracker already exists: <TrackerName>.'
3. DescribeTracker for a deleted tracker (same fixture task#6): compare the 404 message with 'Tracker not found: <TrackerName>.'
4. GetDevicePosition for a device with no position records (location-device-position-roundtrip-and-not-found task#6): compare the 404 message with the fixed sentence 'records not found for given deviceId'.
5. GetMapGlyphs with a FontUnicodeRange that violates ^[0-9]+-[0-9]+\.pbf$ (location-map-data-plane-tile-sprites-glyphs-style task#6): observe the generic constraint sentence and the absent reason/fieldList members.
