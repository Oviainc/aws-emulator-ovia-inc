Issue-Source: trace-sweep-divergence
Issue-Trace: iot-wireless-multicast-group-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-wireless-multicast-group-lifecycle
Issue-Shape-Id: com.amazonaws.iotwireless#iotwireless
Issue-Key: iotwireless|GetMulticastGroup|2xx->2xx|$.LoRaWAN.DefaultSessionParameters
Issue-Key: iotwireless|GetMulticastGroup|2xx->2xx|$.Status
Issue-Key: iotwireless|GetMulticastGroup|2xx->2xx|$.Description
Issue-Key: iotwireless|GetMulticastGroup|4xx->2xx|$.Message
Issue-Key: iotwireless|GetMulticastGroup|4xx->2xx|$.ResourceId
Issue-Key: iotwireless|GetMulticastGroup|4xx->2xx|$.ResourceType
Issue-Key: iotwireless|GetMulticastGroup|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iotwireless|GetMulticastGroup|4xx->2xx|status
Issue-Commit: 0b9721afe99d360c98ec9e05a46069ffbe923470

# GetMulticastGroup drops group state and ignores deletion

> _Note: this issue was identified against the code base at commit `0b9721afe99d360c98ec9e05a46069ffbe923470`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMulticastGroup

## Description
THREE causes in the iotwireless MulticastGroup read path, all one shared fault: the multicast-group record the write operations are given is not persisted and not reflected by the read, so GetMulticastGroup neither round-trips what Create/Update submitted nor observes Delete. All three are net-new; none is covered by the three open iotwireless issues (see the note at the end).

Cause 1 of 3: GetMulticastGroup omits Status and the LoRaWAN default session parameters after a create (fixture iot-wireless-multicast-group-lifecycle, step task#2).
AWS: Smithy: GetMulticastGroup (GET /multicast-groups/{Id}, success 200) outputs Status (target MulticastGroupStatus, string @length 0..2048/0..256) and LoRaWAN (target LoRaWANMulticastGet) whose members include DefaultSessionParameters (target DefaultSessionParametersMulticast, with DlDr @range 0..15 and DlFreq @range 100000000..1000000000), RfRegion, DlClass, NumberOfDevicesRequested and NumberOfDevicesInGroup. CreateMulticastGroup takes the matching LoRaWANMulticast shape as a @required input member carrying the same DefaultSessionParameters, so AWS persists the submitted LoRaWAN block and GetMulticastGroup returns it verbatim: DefaultSessionParameters comes back with the DlDr and DlFreq that were submitted. Status is service-assigned lifecycle state, not a request value: a freshly created group that has no devices/session yet reads back in its initial state, the fixed token 'Pending' in the recording.
Emulator: the 200 response carries neither member - $.Status is absent (reported missing/None) and $.LoRaWAN.DefaultSessionParameters is absent, where the recording has the submitted session parameters and the lifecycle status.
Why: a client that creates a multicast group and reads it back to drive provisioning loses the whole downlink session plan (data rate and frequency) and cannot tell what lifecycle state the group is in. Code that polls GetMulticastGroup until Status leaves its initial value never sees a value at all, and SDK code dereferencing response.LoRaWAN.DefaultSessionParameters.DlFreq gets a null before it can start a multicast session.
Fix: the CreateMulticastGroup handler's state write and the GetMulticastGroup output serializer in the iotwireless service crate - the submitted LoRaWANMulticast block must be stored on the group record, a lifecycle Status must be assigned at create time, and both must be emitted on read.
Traces: iot-wireless-multicast-group-lifecycle#task#2.

Cause 2 of 3: after UpdateMulticastGroup, GetMulticastGroup does not reflect the updated Description, and still omits Status and DefaultSessionParameters (fixture iot-wireless-multicast-group-lifecycle, step task#5).
AWS: Smithy: UpdateMulticastGroup (PATCH /multicast-groups/{Id}, success 204) accepts Name, Description (target Description, string @length 0..2048) and LoRaWAN; GetMulticastGroup outputs Description with the same target. AWS applies the patch to the stored group, so the next GetMulticastGroup returns the Description string exactly as submitted on the update - the value is a request value, not a literal of any one run - alongside the Status and LoRaWAN block of cause 1.
Emulator: the 200 read after the successful update has $.Description absent (missing/None) where the recording has the string the update submitted, and $.Status and $.LoRaWAN.DefaultSessionParameters are absent as in cause 1.
Why: updates are write-only - they return 204 as if applied and are then invisible. A read-modify-write caller re-reads the group, sees no Description, and either re-sends a blank value or treats the update as lost; anything that confirms a mutation landed by reading it back cannot do so, so idempotent reconciliation loops re-patch forever.
Fix: the UpdateMulticastGroup handler's state mutation plus the same GetMulticastGroup output serializer in the iotwireless service crate - the patch must be merged into the stored record and all mutable members must be emitted on read.
Traces: iot-wireless-multicast-group-lifecycle#task#5.

Cause 3 of 3: GetMulticastGroup still answers 200 for a multicast group that was deleted (fixture iot-wireless-multicast-group-lifecycle, step task#8).
AWS: Smithy: GetMulticastGroup declares ResourceNotFoundException (errorKind client, httpError 404) with members Message (@length 0..2048), ResourceId ('Id of the not found resource') and ResourceType. After a successful DeleteMulticastGroup (DELETE /multicast-groups/{Id}, success 204), a subsequent GetMulticastGroup on the same Id is refused with HTTP 404 and that shape populated: ResourceType is the resource kind, the fixed text 'MulticastGroup'; ResourceId is the Id taken from the request URI label (a request value, not a literal from one run); and Message is AWS's fixed sentence for this error class with those two values interpolated - 'Cannot complete the step because resource is not found, with resource type <ResourceType>, resource Id <ResourceId>'. Everything outside those two substitutions is fixed text. The response also carries the x-amzn-errortype header naming the error shape (the fixture does not assert its exact value).
Emulator: the request succeeds with HTTP 200 and a body with no Message, no ResourceId, no ResourceType and no x-amzn-errortype header - the deleted group is still readable.
Why: deletion is not observable through the read path, so a client cannot confirm teardown and cannot use the documented not-found error for control flow. Cleanup loops that poll until 404 never terminate, and callers that branch on ResourceNotFoundException take the success path and go on to operate on a group that no longer exists.
Fix: DeleteMulticastGroup's state mutation and the GetMulticastGroup handler's existence check in the iotwireless service crate - delete must remove (or tombstone) the record, and a miss must raise ResourceNotFoundException with Message/ResourceId/ResourceType populated, HTTP status 404 and the error-type header.
Traces: iot-wireless-multicast-group-lifecycle#task#8.

Relation to open issues: '20261030T072116Z-iotwireless-getdeviceprofile-drops-lorawan-configuration-and-ignores-deletion.md' describes the same shape of fault on the DeviceProfile resource and does not own the MulticastGroup steps; '20261029T171855Z-iotwireless-getdestination-reports-stale-expression-and-words-its-not.md' owns the Destination read path; and '20261029T171855Z-iotwireless-validation-refusals-carry-no-message-and-wrong.md' owns the validation-Message divergences, including CreateMulticastGroup task#6 in this same fixture, which is therefore NOT part of this issue.

## Evidence
Cause 1 - fixture iot-wireless-multicast-group-lifecycle, step task#2, GetMulticastGroup, expected status 200, actual status 200:
  $.Status: missing (expected the initial lifecycle status, 'Pending' in the recording, got None)
  $.LoRaWAN.DefaultSessionParameters: missing (expected the submitted session parameters, {'DlDr': <submitted DlDr>, 'DlFreq': <submitted DlFreq>} - 9 / 923300000 in the recording - got None)
Expected: Status assigned at create time, and every LoRaWANMulticast member supplied to CreateMulticastGroup present on the read with the submitted value. Actual: both absent from the 200 body.

Cause 2 - fixture iot-wireless-multicast-group-lifecycle, step task#5, GetMulticastGroup after a successful UpdateMulticastGroup, expected status 200, actual status 200:
  $.Description: missing (expected the Description string submitted on the update - 'sim2real probe updated description' in the recording - got None)
  $.LoRaWAN.DefaultSessionParameters: missing (got None)
  $.Status: missing (expected 'Pending', got None)
Expected: the read reflects the patched Description and still carries Status and the LoRaWAN block. Actual: the update is invisible and the two create-time members are still absent.

Cause 3 - fixture iot-wireless-multicast-group-lifecycle, step task#8, GetMulticastGroup after DeleteMulticastGroup:
  status: expected 404, got 200
  $.Message: missing (expected 'Cannot complete the step because resource is not found, with resource type MulticastGroup, resource Id <Id>', got None)
  $.ResourceId: missing (expected the deleted group's Id from the request path, got None)
  $.ResourceType: missing (expected 'MulticastGroup', got None)
  $headers.x-amzn-errortype: missing (expected the error-type header, value not asserted by the fixture, got None)
Message breakdown - fixed text: "Cannot complete the step because resource is not found, with resource type " + <ResourceType> + ", resource Id " + <ResourceId>. Variable parts: ResourceType = the fixed resource kind 'MulticastGroup'; ResourceId = the Id path label taken from the request (the recording's '8c5902c3-f9af-448a-8980-f46b15aedab4' is one run's generated id, not part of the requirement). Expected status 404, error shape ResourceNotFoundException.

## Steps to Reproduce
1. Replay the wire corpus simparity-wire-aws-v1 against the emulator (replay_wire_corpus). All three causes are in fixture iot-wireless-multicast-group-lifecycle.
2. Cause 1 (step task#2): POST /multicast-groups with a populated LoRaWAN block including DefaultSessionParameters (DlDr, DlFreq), then GET /multicast-groups/{Id}. Observe the 200 body has no Status and no LoRaWAN.DefaultSessionParameters.
3. Cause 2 (step task#5): PATCH /multicast-groups/{Id} with a new Description (204), then GET /multicast-groups/{Id}. Observe the 200 body has no Description (the update is not reflected), and still no Status or LoRaWAN.DefaultSessionParameters.
4. Cause 3 (step task#8): DELETE /multicast-groups/{Id} (204), then GET /multicast-groups/{Id}. Observe HTTP 200 with an empty body instead of HTTP 404 ResourceNotFoundException carrying Message/ResourceId/ResourceType plus the x-amzn-errortype header.
