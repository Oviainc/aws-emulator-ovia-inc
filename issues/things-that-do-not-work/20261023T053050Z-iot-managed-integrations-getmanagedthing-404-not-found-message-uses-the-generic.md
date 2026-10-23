Issue-Source: trace-sweep-divergence
Issue-Trace: iot-managed-integrations-managed-thing-error-paths
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-managed-integrations-managed-thing-error-paths iot-managed-integrations-managed-thing-lifecycle
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations|GetManagedThing|4xx->4xx|$.Message
Issue-Commit: 9f800acee3d005b691dff51f5c15e7b241ecffe1

# GetManagedThing 404 not-found Message uses the generic service-wide template

> _Note: this issue was identified against the code base at commit `9f800acee3d005b691dff51f5c15e7b241ecffe1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetManagedThing

## Description
This issue has ONE cause. A fixer must close it before submitting.

Cause 1 of 1 - GetManagedThing on a managed thing that does not exist returns the correct 404 ResourceNotFoundException, but the Message member is rendered from the emulator's shared, identifier-interpolating not-found template instead of the fixed text AWS emits for this read operation.

AWS: Smithy: GetManagedThing GET /managed-things/{Identifier} (success code 200), input member Identifier is a required @httpLabel targeting ManagedThingId (@length min 1 max 64, @pattern ^[a-zA-Z0-9:_-]*$); modeled errors include ResourceNotFoundException (errorKind client, httpError 404). The model does not fix the Message text, so: recorded behavior only - the corpus shows the GetManagedThing 404 body carrying a FIXED, request-independent text, 'Managed Thing resource not found.', and it shows it twice in two independent fixtures (an explicit get of an unknown identifier, and a get after a successful delete), with two different request contexts, so the text does not vary with the Identifier. Note this is a DIFFERENT rule from the one AWS uses on the mutating paths of the same resource, where the recorded text is built from the request ("ManagedThing for identifier '<Identifier>' not found").

Emulator: returns status 404 as expected, but with Message built from its single service-wide not-found formatter, which interpolates the requested identifier - recorded example 'Managed thing nonexistentmanagedthing0001 was not found' (replay detail: $.Message value_mismatch, expected 'Managed Thing resource not found.'). Status and error shape are correct; only the Message member diverges.

Why: the emulator applies one not-found template family to every managed-thing operation, where AWS uses an operation-specific one - a constant for the read path and a request-derived one for the mutation paths. A client, test, or log-scraper that distinguishes these paths by the not-found reason text (and anything that asserts the read path's reason text at all) sees a message AWS never produces for GetManagedThing, and it also sees the requested identifier echoed into a message where AWS does not echo it.

Fix: the GetManagedThing handler's not-found path - the ResourceNotFoundException it constructs must use the fixed read-path text rather than the shared identifier-interpolating formatter. This is the same component family as the already-open issue 20261023T023106Z-iot-managed-integrations-error-response-message-text-diverges-from-aws-on.md, which owns the UpdateManagedThing / DeleteManagedThing / enum-validation message causes and whose replay keys do NOT cover GetManagedThing; that issue therefore does not close this one, and whatever formatter it introduces must leave the read path on AWS's constant text.

Traces: iot-managed-integrations-managed-thing-error-paths#task#1 (representative) and iot-managed-integrations-managed-thing-lifecycle#teardown#7 (same cluster, GetManagedThing after delete).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up true, /__admin/health 200, bootstrap_signal false, state_reset_failures 0; 89 steps, 3 passed).

Cluster service=iot-managed-integrations|GetManagedThing|status=404->404|keys=$.Message, verdict mismatch, affected_count 2:
- fixture iot-managed-integrations-managed-thing-error-paths step task#1 (GET /managed-things/nonexistentmanagedthing0001): expected status 404, actual status 404; $.Message value_mismatch - expected 'Managed Thing resource not found.', got 'Managed thing nonexistentmanagedthing0001 was not found'.
- fixture iot-managed-integrations-managed-thing-lifecycle step teardown#7 (GET of the managed thing just deleted): same expected/actual Message divergence, confirming the AWS text is request-independent (different identifier context, identical recorded text).

No other member and no header diverged in this cluster; the HTTP status (404) and the ResourceNotFoundException shape both match. Model references used: GetManagedThing http {method GET, uri /managed-things/{Identifier}, code 200}; ResourceNotFoundException httpError 404, errorKind client, member Message; the model fixes no message text, hence 'recorded behavior only' above.

Not part of this issue (already owned, listed only so the boundary is clear): UpdateManagedThing/DeleteManagedThing/CreateManagedThing/CreateProvisioningProfile $.Message divergences are owned by 20261023T023106Z-iot-managed-integrations-error-response-message-text-diverges-from-aws-on.md, and the UpdateManagedThing 204 Content-Length divergence by 20261022T145600Z-iot-managed-integrations-updatemanagedthing-204-responses-omit-the-content-length-0.md.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture iot-managed-integrations-managed-thing-error-paths step task#1: GET /managed-things/nonexistentmanagedthing0001 (GetManagedThing) for an Identifier that was never created.
3. Observe the response is 404 ResourceNotFoundException (correct) and compare $.Message against the recorded fixed text 'Managed Thing resource not found.'; the emulator instead returns its identifier-interpolated generic text.
4. Replay fixture iot-managed-integrations-managed-thing-lifecycle step teardown#7: after the lifecycle fixture deletes the managed thing it created, GET the same identifier and compare $.Message the same way - the recorded text is identical to step 3's, so the fix must not interpolate the identifier.
5. Both steps must pass before the issue is closed.
