Issue-Source: trace-sweep-divergence
Issue-Trace: iot-wireless-device-profile-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-wireless-device-profile-lifecycle
Issue-Shape-Id: com.amazonaws.iotwireless#iotwireless
Issue-Key: iotwireless|GetDeviceProfile|2xx->2xx|$.LoRaWAN.FactoryPresetFreqsList
Issue-Key: iotwireless|GetDeviceProfile|2xx->2xx|$.LoRaWAN.MaxDutyCycle
Issue-Key: iotwireless|GetDeviceProfile|2xx->2xx|$.LoRaWAN.RfRegion
Issue-Key: iotwireless|GetDeviceProfile|2xx->2xx|$.LoRaWAN.RxDataRate2
Issue-Key: iotwireless|GetDeviceProfile|2xx->2xx|$.LoRaWAN.RxDelay1
Issue-Key: iotwireless|GetDeviceProfile|2xx->2xx|$.LoRaWAN.RxDrOffset1
Issue-Key: iotwireless|GetDeviceProfile|2xx->2xx|$.LoRaWAN.RxFreq2
Issue-Key: iotwireless|GetDeviceProfile|2xx->2xx|$.LoRaWAN.SupportsJoin
Issue-Key: iotwireless|GetDeviceProfile|4xx->2xx|$.Message
Issue-Key: iotwireless|GetDeviceProfile|4xx->2xx|$.ResourceId
Issue-Key: iotwireless|GetDeviceProfile|4xx->2xx|$.ResourceType
Issue-Key: iotwireless|GetDeviceProfile|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iotwireless|GetDeviceProfile|4xx->2xx|status
Issue-Commit: 2294e811bd9f401401d7efd231d6d04e23ec8f30

# GetDeviceProfile drops LoRaWAN configuration and ignores deletion

> _Note: this issue was identified against the code base at commit `2294e811bd9f401401d7efd231d6d04e23ec8f30`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDeviceProfile

## Description
Two causes in the iotwireless DeviceProfile read path: the operation does not round-trip the LoRaWAN configuration it was created with, and it does not observe DeleteDeviceProfile.

Cause 1 of 2: GetDeviceProfile omits the whole LoRaWAN block it was created with (fixture iot-wireless-device-profile-lifecycle, step task#2).
AWS: Smithy: GetDeviceProfile output member LoRaWAN targets LoRaWANDeviceProfile, whose members include RfRegion (@length 0..64), MaxDutyCycle (@range 0..100), SupportsJoin (boolean), RxDelay1 (@range 0..15), RxDrOffset1 (@range 0..7), RxDataRate2 (@range 0..15), RxFreq2 (@range 1000000..16700000) and FactoryPresetFreqsList (@length 0..20 list). CreateDeviceProfile accepts the same LoRaWANDeviceProfile shape as input, so AWS persists the submitted profile and GetDeviceProfile returns it verbatim on the 200 response: every LoRaWAN member supplied at create time comes back with the same value, and the frequency list comes back in the submitted order.
Emulator: the 200 response carries no LoRaWAN content at all — FactoryPresetFreqsList, MaxDutyCycle, RfRegion, RxDataRate2, RxDelay1, RxDrOffset1, RxFreq2 and SupportsJoin are all absent (reported as missing/None) where the recording has the values that were passed to CreateDeviceProfile.
Why: a client that creates a device profile and then reads it back to drive provisioning (region/frequency plan, join mode, RX window parameters) sees an empty profile. Anything that validates a profile before attaching it to a wireless device, or that does read-modify-write on the profile, loses the entire radio configuration; SDK code that dereferences response.LoRaWAN.RfRegion gets a null.
Fix: the CreateDeviceProfile handler's state write and/or the GetDeviceProfile output serializer in the iotwireless service crate — the submitted LoRaWANDeviceProfile must be stored on the device-profile record and emitted on read.
Traces: iot-wireless-device-profile-lifecycle#task#2.

Cause 2 of 2: GetDeviceProfile still answers 200 for a device profile that was deleted (fixture iot-wireless-device-profile-lifecycle, step task#6).
AWS: Smithy: GetDeviceProfile declares ResourceNotFoundException (errorKind client, httpError 404) with members Message (@length 0..2048), ResourceId ("Id of the not found resource") and ResourceType. After a successful DeleteDeviceProfile (HTTP 204), a subsequent GetDeviceProfile on the same Id is refused with HTTP 404 and the error shape populated: ResourceType is the resource kind, here the fixed text "DeviceProfile"; ResourceId is the Id from the request URI label (a value from the request, not a literal from any one run); and Message is AWS's fixed sentence for this class with those two values interpolated — "Cannot complete the step because resource is not found, with resource type <ResourceType>, resource Id <ResourceId>". The fixed text is everything outside the two substitutions. The response also carries the x-amzn-errortype header identifying the error shape (its exact value is not asserted by the fixture).
Emulator: the request succeeds with HTTP 200 and a body with no Message, ResourceId or ResourceType and no x-amzn-errortype header, i.e. the deleted profile is still readable.
Why: delete is not observable through the read path, so a client cannot confirm teardown and cannot use the documented not-found error for control flow. Idempotent cleanup loops that poll until 404 never terminate, and tests that assert a profile is gone after deletion pass against AWS and hang or wrongly succeed here. Consumers that branch on ResourceNotFoundException instead take the success path and operate on a tombstoned profile.
Fix: DeleteDeviceProfile's state mutation and the GetDeviceProfile handler's existence check in the iotwireless service crate — deletion must remove (or tombstone) the record, and a miss must raise ResourceNotFoundException with Message/ResourceId/ResourceType populated and the 404 status plus error-type header.
Traces: iot-wireless-device-profile-lifecycle#task#6.

Neither cause is covered by the two open iotwireless issues ("GetDestination reports stale expression and words its not-found refusal wrongly" and "iotwireless validation refusals carry no Message and wrong enum value set wording"): those own the GetDestination/CreateDestination and validation-message divergences respectively, not the DeviceProfile read path.

## Evidence
Cause 1 — fixture iot-wireless-device-profile-lifecycle, step task#2, GetDeviceProfile, expected status 200, actual status 200:
  $.LoRaWAN.FactoryPresetFreqsList: missing (expected a 9-element frequency list, got None)
  $.LoRaWAN.MaxDutyCycle: missing (expected the submitted duty-cycle value, got None)
  $.LoRaWAN.RfRegion: missing (expected the submitted region, e.g. 'US915', got None)
  $.LoRaWAN.RxDataRate2: missing (got None)
  $.LoRaWAN.RxDelay1: missing (got None)
  $.LoRaWAN.RxDrOffset1: missing (got None)
  $.LoRaWAN.RxFreq2: missing (got None)
  $.LoRaWAN.SupportsJoin: missing (expected True, got None)
Expected: every LoRaWANDeviceProfile member supplied to CreateDeviceProfile present on the read with the submitted value. Actual: the LoRaWAN block is empty.

Cause 2 — fixture iot-wireless-device-profile-lifecycle, step task#6, GetDeviceProfile after DeleteDeviceProfile:
  status: expected 404, got 200
  $.Message: missing (expected 'Cannot complete the step because resource is not found, with resource type DeviceProfile, resource Id <Id>', got None)
  $.ResourceId: missing (expected the deleted profile's Id from the request path, got None)
  $.ResourceType: missing (expected 'DeviceProfile', got None)
  $headers.x-amzn-errortype: missing (expected the error-type header, value not asserted, got None)
Fixed text of the message: "Cannot complete the step because resource is not found, with resource type " + <ResourceType> + ", resource Id " + <ResourceId>. Variable parts: ResourceType = "DeviceProfile" (the resource kind) and ResourceId = the Id path label from the request. Expected status 404, error shape ResourceNotFoundException.

## Steps to Reproduce
1. Replay the wire corpus simparity-wire-aws-v1 against the emulator (replay_wire_corpus).
2. Cause 1: fixture iot-wireless-device-profile-lifecycle, step task#2 — CreateDeviceProfile with a populated LoRaWAN block (RfRegion, MaxDutyCycle, SupportsJoin, RxDelay1, RxDrOffset1, RxDataRate2, RxFreq2, FactoryPresetFreqsList), then GET /device-profiles/{Id}. Observe the 200 body has no LoRaWAN members.
3. Cause 2: fixture iot-wireless-device-profile-lifecycle, step task#6 — DELETE /device-profiles/{Id} (204), then GET /device-profiles/{Id}. Observe HTTP 200 with an empty body instead of HTTP 404 ResourceNotFoundException carrying Message/ResourceId/ResourceType and the x-amzn-errortype header.
