Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: gameliftstreams.add-stream-group-location-duplicate-is-an-idempotent-no-op
Issue-Shape-Id: com.amazonaws.gameliftstreams#GameLiftStreams
Issue-Key: gameliftstreams|AddStreamGroupLocations|ValidationException
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# gameliftstreams AddStreamGroupLocations refuses an idempotent re-add of a location the group already holds (400 ValidationException where AWS answers 200)

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddStreamGroupLocations

## Description
REOPENS 20261022T092721Z-gameliftstreams-addstreamgrouplocations-stub.md: its fix left the call refused.

Cause 1 of 1: AddStreamGroupLocations naming a location the stream group ALREADY holds, at the capacity it already holds, is refused with 400 ValidationException instead of being accepted as an idempotent no-op.

AWS: recorded behavior only for the idempotency rule — the Smithy model (services/service-com-amazonaws-gameliftstreams-gameliftstreams/smithy-model/com-amazonaws-gameliftstreams-gameliftstreams.json) says nothing about duplicate locations; it only gives AddStreamGroupLocations POST /streamgroups/{Identifier}/locations with success status 200, a @required LocationConfigurations list (len 1..100, @uniqueItems) and a @required output pair {Identifier, Locations: LocationStates}. The replayed check (measured against real AWS) shows the re-add succeeding: HTTP 200, the response's Locations list containing a LocationState for the re-added location with Status ACTIVE, the stream group's own Status still ACTIVE on the following GetStreamGroup, and exactly ONE LocationState for that location (no duplicate row). Adding a genuinely new location continues to work the same way (group goes UPDATING_LOCATIONS, the new location activates).
Emulator: answers 400 ValidationException with the emulator-authored message "stream group <group id> already holds location <location name>; use UpdateStreamGroup to change the capacity of a location it holds (Internal ID: <uuid>)" — fixed text AWS never emits, with the group id and the location name interpolated from the request and state and the emulator's per-request internal-id suffix appended. The refusal is raised before any state is written, so the call has no effect at all.
Why: the operation is a natural retry point — a client that retries after a timeout, or that reconciles a declarative desired-location set (IaC/controller loops), replays the same LocationConfigurations. On AWS that replay is harmless; against the emulator every such reconcile fails hard with a 400, which SDK retry logic does not retry, so a workflow that is correct on AWS aborts. Callers also cannot distinguish this 400 from a genuinely invalid location.
Fix: the AddStreamGroupLocations handler's pre-write validation — the `group.has_location(...)` branch must not refuse; a configuration naming a location the group already holds must be a no-op for that entry (leave the stored LocationSlot and the group's Status untouched, add no second slot) and the location must still be reported in the response's Locations list with its current status. The duplicate-within-one-request check (the same location twice in one LocationConfigurations list) is a separate rule and is not what this covers.
Traces: gameliftstreams.add-stream-group-location-duplicate-is-an-idempotent-no-op.

## Evidence
Replay of suite simparity-behavioral-aws-v1, check gameliftstreams.add-stream-group-location-duplicate-is-an-idempotent-no-op (verdict: error):

botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the AddStreamGroupLocations operation: stream group sg-G1aVd1 already holds location us-west-2; use UpdateStreamGroup to change the capacity of a location it holds (Internal ID: 857204e8-4c50-1034-09a4-a6f5afb8a473)

Expected (per the check, measured against real AWS): HTTP 200; readd_response["Locations"] contains the re-added location with Status == "ACTIVE"; GetStreamGroup still reports Status == "ACTIVE" and exactly one LocationState for that location.
Actual: HTTP 400 ValidationException, no response body members, nothing in the Locations list.

Source: the refusal is raised in services/service-com-amazonaws-gameliftstreams-gameliftstreams/src/ops/add_stream_group_locations.rs lines 237-248 (`if group.has_location(&configuration.location_name) { ... ValidationException ... }`).
Model evidence: AddStreamGroupLocations http {method: POST, uri: /streamgroups/{Identifier}/locations, code: 200}; output {Identifier required, Locations: LocationStates required}; errors include ValidationException (400) but the model states no duplicate-location rule.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. CreateStreamGroup(Description=<any>, StreamClass="gen4n_high", LocationConfigurations=[{LocationName: <request region>, MaximumCapacity: 2}]) and poll GetStreamGroup until Status == ACTIVE.
3. Call AddStreamGroupLocations(Identifier=<stream group ARN>, LocationConfigurations=[{LocationName: <same region>, MaximumCapacity: 2}]) — i.e. re-add the location the group already holds at the same capacity.
4. Observe HTTP 400 ValidationException "stream group <id> already holds location <region>; use UpdateStreamGroup to change the capacity of a location it holds (Internal ID: ...)" where AWS answers HTTP 200 with that location reported ACTIVE in Locations, the group still ACTIVE, and a single LocationState for it.
