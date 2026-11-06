Issue-Source: trace-sweep-divergence
Issue-Trace: location-geofence-collection-crud-and-conflict
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: location-geofence-collection-crud-and-conflict
Issue-Shape-Id: com.amazonaws.location#LocationService
Issue-Key: location|DescribeGeofenceCollection|2xx->2xx|$.Tags
Issue-Key: location|CreateGeofenceCollection|4xx->4xx|$.message
Issue-Key: location|DescribeGeofenceCollection|4xx->2xx|$.message
Issue-Key: location|DescribeGeofenceCollection|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: location|DescribeGeofenceCollection|4xx->2xx|status
Issue-Commit: 3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713

# Location geofence collection plane diverges: Tags omitted, emulator-authored conflict text, delete never retires the collection

> _Note: this issue was identified against the code base at commit `3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeGeofenceCollection

## Description
Three causes, one shared fault: the geofence-collection resource plane in the location emulator does not render, refuse or retire a collection the way AWS does. The read path drops a modelled member, the conflict refusal is worded from the crate's generic resource template instead of the service's sentence, and the delete leaves the collection readable so the modelled ResourceNotFoundException is never raised. The HTTP statuses are right in causes 1 and 2; the bodies are not, and in cause 3 the status itself is wrong.

Cause 1 of 3: DescribeGeofenceCollection omits the Tags member for an untagged collection.
AWS: Smithy: DescribeGeofenceCollection GET /geofencing/v0/collections/{CollectionName}, output DescribeGeofenceCollectionResponse with member Tags (target TagMap, map, @length max 50; TagMap key TagKey @length 1..128, value TagValue @length 0..256). The recording for a collection created with no tags carries Tags as an empty JSON object, so AWS projects the collection's tag map unconditionally rather than omitting it when empty.
Emulator: the Tags member is absent from the 200 body entirely (expected {}, got no member).
Why: because AWS answers an untagged collection with an empty map rather than omitting the member, a client cannot distinguish "this collection has no tags" from "this field was not returned"; code that iterates response.Tags without a null guard breaks, and a read-after-create round-trip comparison (create with Tags, describe, compare) fails on a member that should always be present.
Fix: the DescribeGeofenceCollection response serializer and the geofence-collection state projection in the location service crate — always project the collection's tag map, as an empty map when the collection carries no tags. The same omission is already filed for the route-calculator read path in open issue 20261031T180556Z-location-route-calculator-plane-diverges-epoch-timestamps-missing.md and for the tracker read paths in 20261029T234705Z-location-tracker-read-responses-serialize-timestamps-as-epoch.md; neither covers DescribeGeofenceCollection.
Traces: location-geofence-collection-crud-and-conflict#task#2.

Cause 2 of 3: CreateGeofenceCollection duplicate-name conflict message is emulator-authored.
AWS: Smithy: CreateGeofenceCollection POST /geofencing/v0/collections (@idempotent), error ConflictException (client error, httpError 409, documented "The request was unsuccessful because of a conflict") with one required member Message serialized with jsonName "message". Re-creating a collection whose CollectionName already exists answers 409 with message "Geofence collection already exists: <CollectionName>." — the fixed text is "Geofence collection already exists: " plus a terminating period, and the only variable part is the CollectionName taken from the request body. The collection name in the recording is a per-run probe value and is not part of the requirement.
Emulator: 409 (status correct) with message 'Resource of type "GeofenceCollection" with name "<CollectionName>" already exists.' — the crate's generic resource-type-and-name template, which the service never emits.
Why: the status matches so a client branches correctly, but any client, test or operator-facing surface that matches or displays the conflict wording sees text AWS does not produce. The same generic template is already filed for CreateTracker in open issue 20261029T234705Z-location-refusal-bodies-are-emulator-authored-and-omit.md; that issue covers CreateTracker, DescribeTracker, GetDevicePosition and GetMapGlyphs and does not cover this operation.
Fix: the CreateGeofenceCollection handler's conflict path in the location service crate — raise ConflictException with the service's own sentence instead of the shared generic resource template.
Traces: location-geofence-collection-crud-and-conflict#task#4.

Cause 3 of 3: after DeleteGeofenceCollection succeeds, DescribeGeofenceCollection still answers 200 with the deleted collection instead of 404 ResourceNotFoundException.
AWS: Smithy: DeleteGeofenceCollection DELETE /geofencing/v0/collections/{CollectionName} (@idempotent) retires the collection; DescribeGeofenceCollection then models ResourceNotFoundException (client error, httpError 404, required Message with jsonName "message", documented "The resource that you've entered was not found in your AWS account"), and the REST-JSON refusal carries the x-amzn-errortype response header naming that error shape. The recorded refusal body message is "Geofence collection not found: <CollectionName>." — fixed text "Geofence collection not found: " plus the CollectionName taken from the request URI label plus a period; the name in the recording is a per-run probe value.
Emulator: the describe issued after the successful delete returns HTTP 200 carrying the collection's body — no message member and no x-amzn-errortype header — so the delete left the resource readable (it either never removed it from state or the read path does not consult the deletion).
Why: a client that deletes a collection and polls describe to confirm removal never observes it go away, so teardown loops, idempotent re-create flows and existence checks all conclude the wrong thing; callers treating 200 as liveness keep addressing a collection AWS has already retired, and the modelled 404 error path is never exercised. The identical delete-does-not-take-effect fault is filed for route calculators in open issue 20261031T180556Z-location-route-calculator-plane-diverges-epoch-timestamps-missing.md, which does not cover the geofence-collection resource.
Fix: the DeleteGeofenceCollection handler and the geofence-collection state store in the location service crate — remove the collection from state on delete, and have the describe read path raise ResourceNotFoundException (HTTP 404, x-amzn-errortype: ResourceNotFoundException, message worded as above) for a name that is absent.
Traces: location-geofence-collection-crud-and-conflict#task#6.

## Evidence
Replay of fixture location-geofence-collection-crud-and-conflict (trace set simparity-wire-aws-v1) against the emulator built from this checkout.

Cause 1 — step task#2 (DescribeGeofenceCollection, 200 expected and 200 returned). Divergence key service=location|DescribeGeofenceCollection|status=200->200|keys=$.Tags. Detail: $.Tags: missing (expected {}, got None).

Cause 2 — step task#4 (CreateGeofenceCollection re-creating an existing collection, 409 expected and 409 returned). Divergence key service=location|CreateGeofenceCollection|status=409->409|keys=$.message. Detail: $.message value_mismatch — expected 'Geofence collection already exists: <CollectionName>.' (recorded as 'Geofence collection already exists: sim2real-probe-location-collection-0d449a48.'; fixed text 'Geofence collection already exists: ' + the requested CollectionName + '.'), got 'Resource of type "GeofenceCollection" with name "<CollectionName>" already exists.'

Cause 3 — step task#6 (DescribeGeofenceCollection after the collection was deleted). Divergence key service=location|DescribeGeofenceCollection|status=404->200|keys=$.message,$headers.x-amzn-errortype,status. Detail: status expected 404, got 200; $.message expected 'Geofence collection not found: <CollectionName>.' (recorded as 'Geofence collection not found: sim2real-probe-location-collection-0d449a48.'; fixed text 'Geofence collection not found: ' + the requested CollectionName + '.'), got None; response header x-amzn-errortype expected present naming the error shape, got None.

In all three the recorded collection name is a per-run probe value, not a requirement; the requirements are the status, the error code/shape and the message template.

## Steps to Reproduce
1. Replay fixture location-geofence-collection-crud-and-conflict against the emulator (git_clone, then replay_wire_corpus).
2. Step task#1 — POST /geofencing/v0/collections (CreateGeofenceCollection) for a fresh CollectionName with no Tags.
3. Step task#2 — GET /geofencing/v0/collections/{CollectionName} (DescribeGeofenceCollection): observe the Tags member missing from the 200 body where AWS returns an empty map {}.
4. Step task#4 — POST /geofencing/v0/collections again with the SAME CollectionName: the status is 409 but the body message is 'Resource of type "GeofenceCollection" with name "<CollectionName>" already exists.' instead of 'Geofence collection already exists: <CollectionName>.'
5. Step task#5 — DELETE /geofencing/v0/collections/{CollectionName} (DeleteGeofenceCollection) for that collection.
6. Step task#6 — GET /geofencing/v0/collections/{CollectionName} again: observe HTTP 200 with the deleted collection's body instead of HTTP 404 ResourceNotFoundException with header x-amzn-errortype: ResourceNotFoundException and message 'Geofence collection not found: <CollectionName>.'
