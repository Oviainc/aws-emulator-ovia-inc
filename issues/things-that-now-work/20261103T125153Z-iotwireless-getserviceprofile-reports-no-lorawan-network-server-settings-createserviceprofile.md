Issue-Source: trace-sweep-divergence
Issue-Trace: iot-wireless-service-profile-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-wireless-service-profile-lifecycle
Issue-Shape-Id: com.amazonaws.iotwireless#iotwireless
Issue-Key: iotwireless|GetServiceProfile|2xx->2xx|$.LoRaWAN.AdrAlgorithm
Issue-Key: iotwireless|GetServiceProfile|2xx->2xx|$.LoRaWAN.DevStatusReqFreq
Issue-Key: iotwireless|GetServiceProfile|2xx->2xx|$.LoRaWAN.DlBucketSize
Issue-Key: iotwireless|GetServiceProfile|2xx->2xx|$.LoRaWAN.DlRate
Issue-Key: iotwireless|GetServiceProfile|2xx->2xx|$.LoRaWAN.MinGwDiversity
Issue-Key: iotwireless|GetServiceProfile|2xx->2xx|$.LoRaWAN.TargetPer
Issue-Key: iotwireless|GetServiceProfile|2xx->2xx|$.LoRaWAN.UlBucketSize
Issue-Key: iotwireless|GetServiceProfile|2xx->2xx|$.LoRaWAN.UlRate
Issue-Key: iotwireless|GetServiceProfile|2xx->2xx|$.LoRaWAN.UplinkHistoryLen
Issue-Key: iotwireless|CreateServiceProfile|4xx->2xx|$.Message
Issue-Key: iotwireless|CreateServiceProfile|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iotwireless|CreateServiceProfile|4xx->2xx|status
Issue-Key: iotwireless|GetServiceProfile|4xx->2xx|$.Message
Issue-Key: iotwireless|GetServiceProfile|4xx->2xx|$.ResourceId
Issue-Key: iotwireless|GetServiceProfile|4xx->2xx|$.ResourceType
Issue-Key: iotwireless|GetServiceProfile|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iotwireless|GetServiceProfile|4xx->2xx|status
Issue-Commit: ac0b6fedb2c1d90bf91ede974c0347712fb6fd25

# GetServiceProfile reports no LoRaWAN network-server settings, CreateServiceProfile skips the DrMin/DrMax check, and a deleted profile stays readable

> _Note: this issue was identified against the code base at commit `ac0b6fedb2c1d90bf91ede974c0347712fb6fd25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetServiceProfile

## Description
THREE net-new root causes in the iotwireless service-profile resource, all on the same fixture. They share one fault: the emulator's service-profile record is a thin echo of the create request, so nothing that AWS's network server adds (defaults), enforces (cross-member rules) or removes (deletion) is modelled. Not covered by any open iotwireless issue: 20261029T171855Z-iotwireless-validation-refusals-carry-no-message-and-wrong.md is about the generic '1 validation error detected: ...' constraint template on other Create* operations, and the GetDeviceProfile / GetMulticastGroup / GetWirelessGateway issues are other resources.

Cause 1 of 3: GetServiceProfile drops the whole network-server half of LoRaWANGetServiceProfileInfo. [fixture iot-wireless-service-profile-lifecycle step task#2]
AWS: GetServiceProfile (Smithy: GET /service-profiles/{Id}, success 200; output member LoRaWAN -> LoRaWANGetServiceProfileInfo) reports a SUPERSET of what CreateServiceProfile's LoRaWANServiceProfile can set: the model declares UlRate, UlBucketSize, UlRatePolicy, DlRate, DlBucketSize, DlRatePolicy, DevStatusReqFreq, ReportDevStatus*, ChannelMask, NwkGeoLoc, TargetPer (@range 0..100), MinGwDiversity (@range 1..100), HrAllowed in addition to the create-settable AddGwMetadata/DrMin/DrMax/PrAllowed/RaAllowed/TxPowerIndex*/NbTrans*. AWS populates the network-server members with the account's service-profile defaults even when the create named none of them; the recording for a default-created profile shows UlRate 60, UlBucketSize 4096, DlRate 60, DlBucketSize 4096, DevStatusReqFreq 24, MinGwDiversity 1, TargetPer 5, plus AdrAlgorithm 'ORIGINAL' and UplinkHistoryLen 20 — those last two are recorded behavior only, the vendored model does not declare them on this shape. These are server-side constants, not per-run values.
Emulator: every one of them comes back absent, and TargetPer comes back 0 — the handler projects only the members the create shape shares and leaves the rest at the struct default (see Traces for the projection site), so a boolean/int member defaults to 0/false and every optional one to null.
Why: a client reading rate policing, ADR or status-request configuration off a profile it just created gets nulls where AWS gives numbers, and a client that branches on TargetPer reads 0 ("no packet-error target") instead of AWS's 5 — a wrong value is worse than a missing one because it validates.
Fix: the service-profile state defaults plus GetServiceProfile's output projection — a created profile must persist AWS's network-server defaults for every member the request omitted, and the operation must project all of them.

Cause 2 of 3: CreateServiceProfile accepts a profile whose DrMin exceeds its DrMax. [fixture iot-wireless-service-profile-lifecycle step task#4]
AWS: CreateServiceProfile refuses a LoRaWAN block whose DrMin is greater than the effective DrMax with HTTP 400 ValidationException (Smithy: CreateServiceProfile errors include ValidationException @httpError 400, member Message; LoRaWANServiceProfile.DrMin/DrMax each @range 0..15, DrMax defaulting to 0 when omitted) and creates nothing. Message template: "DrMin <drMin> should not be larger than DrMax <drMax>" — the words 'DrMin', 'should not be larger than' and 'DrMax' are fixed text; both numbers are taken from the request (or, for an omitted member, from the modelled default the service applies). The recorded step sends DrMin 15 with no DrMax and is refused with "DrMin 15 should not be larger than DrMax 0". The cross-member rule itself is recorded behavior only: no modelled trait expresses it.
Emulator: the create SUCCEEDS with HTTP 201 and returns an Arn/Id; the handler validates only forbidden Name characters, the idempotency token and a store ceiling, and never compares DrMin against DrMax.
Why: a client that relies on AWS rejecting an inverted data-rate window never sees its error path, and later reads a profile real IoT Wireless would never have created; downstream code computing an ADR range from DrMin..DrMax gets an empty/inverted interval.
Fix: CreateServiceProfile's input validation — compare DrMin with the effective DrMax before minting the profile and return the modelled ValidationException with the message above.

Cause 3 of 3: GetServiceProfile still answers 200 for a profile that has been deleted, and its not-found body is not AWS's. [fixture iot-wireless-service-profile-lifecycle step task#6]
AWS: reading a service profile that no longer exists is HTTP 404 ResourceNotFoundException (Smithy: GetServiceProfile errors include ResourceNotFoundException @httpError 404 with members Message, ResourceId, ResourceType; x-amzn-errortype header present). Body: ResourceType "ServiceProfile", ResourceId the requested profile id, and Message the fixed sentence "Cannot complete the step because resource is not found, with resource type ServiceProfile, resource Id <id>" — everything but the trailing id (and the resource-type word) is fixed text; the id is the one from the request path.
Emulator: the step returns HTTP 200 with the full profile body — the record is never removed, because DeleteServiceProfile is an unimplemented scaffold stub that answers 501 (the blocking step is covered by its own unimplemented-operation issue; what is filed here is the Get). Independently, the 404 this handler DOES build for an unknown id carries the emulator's own prose "ServiceProfile <id> not found" instead of AWS's sentence, so the status will still diverge in body once the delete lands.
Why: a client cannot observe deletion at all — a resource it destroyed keeps answering 200, so cleanup verification and "recreate after delete" flows silently pass against the emulator and fail against AWS; and a client matching the refusal text to distinguish a missing profile from a throttle sees an unknown sentence.
Fix: the service-profile state (delete must remove the record so this operation's lookup misses) and GetServiceProfile's not-found path, which must emit AWS's ResourceNotFoundException message/ResourceId/ResourceType.

Traces: iot-wireless-service-profile-lifecycle#task#2 (cause 1), #task#4 (cause 2), #task#6 (cause 3). Sites: services/service-com-amazonaws-iotwireless-iotwireless/src/ops/get_service_profile.rs:178-193 (the `reported_settings` projection that fills everything else with Default) and :153-161 (the non-AWS not-found message); src/ops/create_service_profile.rs:166-246 (all validation present — no DrMin/DrMax comparison); src/ops/delete_service_profile.rs:159 (not_implemented).

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 88 steps, 31 passed). Fixture iot-wireless-service-profile-lifecycle, three diverging steps:

1) step task#2 GetServiceProfile — expected 200, got 200, body diverges:
   $.LoRaWAN.AdrAlgorithm: expected 'ORIGINAL', got None
   $.LoRaWAN.DevStatusReqFreq: expected 24, got None
   $.LoRaWAN.DlBucketSize: expected 4096, got None
   $.LoRaWAN.DlRate: expected 60, got None
   $.LoRaWAN.MinGwDiversity: expected 1, got None
   $.LoRaWAN.TargetPer: expected 5, got 0
   $.LoRaWAN.UlBucketSize: expected 4096, got None
   $.LoRaWAN.UlRate: expected 60, got None
   $.LoRaWAN.UplinkHistoryLen: expected 20, got None

2) step task#4 CreateServiceProfile — expected 400, got 201:
   status: expected 400, got 201
   $.Message: expected 'DrMin 15 should not be larger than DrMax 0', got None
   $headers.x-amzn-errortype: expected (shaped, not asserted), got None

3) step task#6 GetServiceProfile after DeleteServiceProfile — expected 404, got 200:
   status: expected 404, got 200
   $.Message: expected 'Cannot complete the step because resource is not found, with resource type ServiceProfile, resource Id <profileId>', got None
   $.ResourceId: expected the deleted profile id, got None
   $.ResourceType: expected 'ServiceProfile', got None
   $headers.x-amzn-errortype: expected (shaped, not asserted), got None

Model evidence (services/service-com-amazonaws-iotwireless-iotwireless/smithy-model/com-amazonaws-iotwireless-iotwireless.json): GetServiceProfile GET /service-profiles/{Id} 200, errors AccessDenied(403)/InternalServer(500)/ResourceNotFound(404)/Throttling(429)/Validation(400); LoRaWANGetServiceProfileInfo{UlRate,UlBucketSize,UlRatePolicy,DlRate,DlBucketSize,DlRatePolicy,AddGwMetadata,DevStatusReqFreq,ReportDevStatusBattery,ReportDevStatusMargin,DrMin,DrMax,ChannelMask,PrAllowed,HrAllowed,RaAllowed,NwkGeoLoc,TargetPer,MinGwDiversity,TxPowerIndexMin/Max,NbTransMin/Max}; LoRaWANServiceProfile{AddGwMetadata,DrMin,DrMax,PrAllowed,RaAllowed,TxPowerIndex*,NbTrans*}; CreateServiceProfile POST /service-profiles 201 with ValidationException(400); ResourceNotFoundException{Message,ResourceId,ResourceType} @httpError 404. AdrAlgorithm and UplinkHistoryLen are in the recording but not in this vendored shape (recorded behavior only).

## Steps to Reproduce
1. Build and boot the emulator from this checkout; replay wire fixture iot-wireless-service-profile-lifecycle of trace set simparity-wire-aws-v1 (service iotwireless).
2. Cause 1 — step task#2: CreateServiceProfile(Name=<any>, LoRaWAN={}) then GetServiceProfile(Id=<minted id>). The 200 body's LoRaWAN carries only the create-settable members; UlRate/UlBucketSize/DlRate/DlBucketSize/DevStatusReqFreq/MinGwDiversity/AdrAlgorithm/UplinkHistoryLen are absent and TargetPer is 0 instead of AWS's 5.
3. Cause 2 — step task#4: CreateServiceProfile(Name=<any>, LoRaWAN={DrMin: 15}) (DrMax omitted, so 0). The emulator answers 201 with an Arn/Id; AWS answers 400 ValidationException with Message "DrMin 15 should not be larger than DrMax 0" and creates nothing.
4. Cause 3 — step task#6: DeleteServiceProfile(Id=<the profile>) (currently 501, its own stub issue), then GetServiceProfile(Id=<same id>). The emulator answers 200 with the profile body; AWS answers 404 ResourceNotFoundException with ResourceType 'ServiceProfile', ResourceId=<id> and Message "Cannot complete the step because resource is not found, with resource type ServiceProfile, resource Id <id>". Also call GetServiceProfile with an id that never existed: the emulator's 404 Message is "ServiceProfile <id> not found" instead of that sentence.
