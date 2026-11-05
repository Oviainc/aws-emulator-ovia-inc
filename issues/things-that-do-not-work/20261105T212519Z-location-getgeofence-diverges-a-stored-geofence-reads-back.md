Issue-Source: trace-sweep-divergence
Issue-Trace: location-geofence-put-get-list-and-not-found
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: location-geofence-put-get-list-and-not-found
Issue-Shape-Id: com.amazonaws.location#LocationService
Issue-Key: location|GetGeofence|2xx->4xx|$.CreateTime
Issue-Key: location|GetGeofence|2xx->4xx|$.GeofenceId
Issue-Key: location|GetGeofence|2xx->4xx|$.Geometry
Issue-Key: location|GetGeofence|2xx->4xx|$.Status
Issue-Key: location|GetGeofence|2xx->4xx|$.UpdateTime
Issue-Key: location|GetGeofence|2xx->4xx|status
Issue-Key: location|GetGeofence|4xx->4xx|$.message
Issue-Commit: e2bde66c8eefd1bf5737c2443dec404d65ff4545

# Location GetGeofence diverges: a stored geofence reads back 404, and the not-found message is emulator-authored

> _Note: this issue was identified against the code base at commit `e2bde66c8eefd1bf5737c2443dec404d65ff4545`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetGeofence

## Description
Two net-new root causes in the location geofencing data plane, both in GetGeofence (2 causes). Neither is covered by an existing open issue: the open issues 20261105T012835Z-location-geofence-collection-plane-diverges-tags-omitted-emulator.md (geofence COLLECTION control plane) and 20261029T234705Z-location-refusal-bodies-are-emulator-authored-and-omit.md (ValidationException Reason/FieldList on other operations) own different clusters and do not carry these replay keys.

Cause 1 of 2: GetGeofence cannot read back a geofence that PutGeofence just accepted.
AWS: Smithy: GetGeofence GET /geofencing/v0/collections/{CollectionName}/geofences/{GeofenceId}, http code 200, output GetGeofenceResponse with REQUIRED members GeofenceId (Id), Geometry (GeofenceGeometry), Status (String), CreateTime and UpdateTime (Timestamp, @timestampFormat date-time), plus optional GeofenceProperties. PutGeofence PUT on the same URI creates or replaces the geofence and returns GeofenceId/CreateTime/UpdateTime, so an immediately following GetGeofence for the same CollectionName+GeofenceId MUST answer 200 with the geometry that was stored and Status 'ACTIVE'; ResourceNotFoundException (404) is modeled only for a collection or geofence that does not exist.
Emulator: after a successful PutGeofence in the same fixture, GetGeofence for that same collection/geofence answers 404 and the body carries none of the modeled output members — GeofenceId, Geometry, Status, CreateTime and UpdateTime are all absent (replay detail: 'status: expected 200, got 404; $.GeofenceId missing; $.Geometry missing; $.Status missing; $.CreateTime missing; $.UpdateTime missing').
Why: a client that writes a geofence and then reads it (or polls it until ACTIVE) concludes the write was lost and the geofence does not exist. Any put-then-get or put-then-evaluate workflow — geofence provisioning, idempotent re-put, or verifying a polygon before attaching device tracking — fails against the emulator while succeeding on AWS. The 404 is also indistinguishable from a genuinely missing geofence, so retry logic cannot recover.
Fix: the location crate's geofencing handlers — PutGeofence must persist the geofence (collection name + geofence id key, geometry, Status, CreateTime/UpdateTime) into service state, and GetGeofence must look it up from that same keyspace and serialize the required output members (timestamps as date-time, Status 'ACTIVE'); this is a state/handler defect, not a serializer-only one, since ListGeofences in the same fixture must see the same record.
Traces: location-geofence-put-get-list-and-not-found#task#3.

Cause 2 of 2: the GetGeofence not-found refusal message is emulator-authored instead of AWS's wording.
AWS: Smithy: GetGeofence error ResourceNotFoundException, httpError 404, required member Message (jsonName 'message'); the recording shows AWS wording for this error class as the fixed text 'Geofence not found: ' followed by the resource name taken from the request and a terminating period — i.e. 'Geofence not found: <name>.' where only <name> is variable (the geofence/collection name from the request path; the recorded run's 'sim2real-probe-location-missing-gf-64c68337' is that run's value, not part of the template). Status and error code themselves are correct (404 / ResourceNotFoundException).
Emulator: the same 404 carries a generic, locally composed sentence instead: 'Resource of type "Geofence" with name "<name>" cannot be found.' The fixed text differs entirely from AWS's ('Resource of type ... cannot be found.' vs 'Geofence not found: ...'), even though the variable name is substituted correctly.
Why: clients, test suites and SDK-level error matchers that assert on or parse the error message text (and humans reading logs) see wording that no AWS region emits, so message-based assertions and log-scraping diverge; it also signals a generic resource-not-found helper is being used in place of the per-operation AWS wording, which means every other location not-found message is suspect for the same reason.
Fix: the location crate's GetGeofence handler/error construction — build ResourceNotFoundException.Message from the AWS template for this error class ('Geofence not found: ' + the requested name + '.') rather than the shared generic 'Resource of type "X" with name "Y" cannot be found.' formatter.
Traces: location-geofence-put-get-list-and-not-found#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (score 27.7%, 11 divergence clusters, all service=location).

Cause 1 — fixture location-geofence-put-get-list-and-not-found, step task#3, operation GetGeofence:
  expected_status 200, actual_status 404
  detail: status: expected 200, got 404
          $.GeofenceId: missing (expected 'gf1', got None)
          $.Geometry: missing (expected {'Polygon': [[[-122.34, 47.6], [-122.33, 47.6], [-122.33, 47.61], [-122.34, 47.61], [-122.34, 47.6]]]}, got None)
          $.Status: missing (expected 'ACTIVE', got None)
          $.CreateTime: missing (expected '<shaped-not-asserted>', got None)
          $.UpdateTime: missing (expected '<shaped-not-asserted>', got None)
  (the preceding PutGeofence step in the same fixture was accepted)

Cause 2 — fixture location-geofence-put-get-list-and-not-found, step task#6, operation GetGeofence:
  expected_status 404, actual_status 404
  detail: $.message: value_mismatch
          expected 'Geofence not found: sim2real-probe-location-missing-gf-64c68337.'
          actual   'Resource of type "Geofence" with name "sim2real-probe-location-missing-gf-64c68337" already/cannot be found' -> exact actual text: 'Resource of type "Geofence" with name "sim2real-probe-location-missing-gf-64c68337" cannot be found.'
  Fixed text is the divergence; the name inside it is a per-run value supplied by the request.

Model evidence: Smithy GetGeofence (services/service-com-amazonaws-location-locationservice/smithy-model/com-amazonaws-location-locationservice.json) — output requires GeofenceId, Geometry, Status, CreateTime, UpdateTime; errors include ResourceNotFoundException (httpError 404) whose required Message member serializes as 'message'.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay fixture location-geofence-put-get-list-and-not-found.
3. Cause 1 — step task#3: after the fixture's PutGeofence (PUT /geofencing/v0/collections/<collection>/geofences/gf1 with a Polygon geometry) succeeds, issue GET /geofencing/v0/collections/<collection>/geofences/gf1. Expected: 200 with GeofenceId 'gf1', the stored Polygon in Geometry, Status 'ACTIVE' and date-time CreateTime/UpdateTime. Actual: 404 with no modeled output members.
4. Cause 2 — step task#6: issue GET /geofencing/v0/collections/.../geofences/<name that does not exist>. Expected: 404 ResourceNotFoundException with message 'Geofence not found: <name>.'. Actual: 404 with message 'Resource of type "Geofence" with name "<name>" cannot be found.'
