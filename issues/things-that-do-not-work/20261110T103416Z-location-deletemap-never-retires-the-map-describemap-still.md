Issue-Source: trace-sweep-divergence
Issue-Trace: location-map-crud-and-conflict
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: location-map-crud-and-conflict
Issue-Shape-Id: com.amazonaws.location#LocationService
Issue-Key: location|DescribeMap|4xx->2xx|$.message
Issue-Key: location|DescribeMap|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: location|DescribeMap|4xx->2xx|status
Issue-Commit: 223a15a94a939697007a456b476d3266d8508e9d

# Location DeleteMap never retires the map: DescribeMap still answers 200 instead of 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `223a15a94a939697007a456b476d3266d8508e9d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeMap

## Description
One cause: the map resource plane in the location emulator does not retire a map on delete, so a read-after-delete still serves the resource.

Cause 1 of 1: after DeleteMap succeeds, DescribeMap still answers HTTP 200 with the deleted map's body instead of the modelled 404 ResourceNotFoundException.
AWS: Smithy: DeleteMap DELETE /maps/v0/maps/{MapName} (@idempotent, MapName is an @httpLabel ResourceName, @length 1-100, @pattern ^[-._\w]+$) retires the map; DescribeMap GET /maps/v0/maps/{MapName} (@readonly) models the errors AccessDeniedException (403), ValidationException (400), ResourceNotFoundException (client error, HTTP 404), ThrottlingException (429) and InternalServerException (500). The ResourceNotFoundException body carries its Message member serialized with jsonName "message", and a REST-JSON refusal also carries the x-amzn-errortype response header naming the error shape. The recorded refusal body message is "Map not found: <MapName>." — the fixed text "Map not found: " followed by the MapName taken from the request URI label and a trailing period; the concrete name in the recording is a per-run probe value and is not part of the requirement.
Emulator: the DescribeMap issued after the map was deleted returns HTTP 200 with the map's describe body — no `message` member and no x-amzn-errortype header — so the delete left the map readable: either DeleteMap never removed it from the map state store, or the describe read path does not consult the deletion.
Why: a client that deletes a map and polls DescribeMap to confirm removal never observes the resource go away, so teardown loops, idempotent re-create flows and "does this map still exist?" checks all conclude the wrong thing; callers that treat 200 as liveness keep using a map AWS has already retired, and the modelled 404 error path is never exercised, so client-side ResourceNotFoundException handling goes untested.
Fix: the DeleteMap handler and the map state store in the location service crate — remove the map from state on delete, and have the DescribeMap read path raise ResourceNotFoundException for an absent name, serialized as HTTP 404 with the x-amzn-errortype header naming ResourceNotFoundException and the message worded as above. Note the open issue 20261031T180556Z-location-route-calculator-plane-diverges-epoch-timestamps-missing.md reports the same class of fault for DeleteRouteCalculator/DescribeRouteCalculator and 20261029T234705Z-location-refusal-bodies-are-emulator-authored-and-omit.md covers emulator-authored refusal wording on other location operations; neither covers the map resource plane or this operation.
Traces: location-map-crud-and-conflict#task#6.

## Evidence
Fixture location-map-crud-and-conflict, step task#6 (DescribeMap, issued after the map was deleted earlier in the same fixture): status expected 404, got 200; $.message expected 'Map not found: <MapName>.' (recorded as 'Map not found: sim2real-probe-location-map-1c6a2523.' — fixed text plus the requested map name), got None; response header x-amzn-errortype expected to be present naming the error shape, got None. Divergence key: service=location|DescribeMap|status=404->200|keys=$.message,$headers.x-amzn-errortype,status.

## Steps to Reproduce
1. Replay fixture location-map-crud-and-conflict against the emulator.
2. Create a map (POST /maps/v0/maps, CreateMap) and confirm DescribeMap answers 200.
3. Delete it (DELETE /maps/v0/maps/{MapName}, DeleteMap) — the emulator reports success.
4. Step task#6 — GET /maps/v0/maps/{MapName} (DescribeMap) again: observe HTTP 200 with the deleted map's body instead of HTTP 404 ResourceNotFoundException with the x-amzn-errortype header set and body message 'Map not found: <MapName>.'
