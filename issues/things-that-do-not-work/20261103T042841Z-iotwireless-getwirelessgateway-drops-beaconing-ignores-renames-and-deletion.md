Issue-Source: trace-sweep-divergence
Issue-Trace: iot-wireless-wireless-gateway-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-wireless-wireless-gateway-lifecycle
Issue-Shape-Id: com.amazonaws.iotwireless#iotwireless
Issue-Key: iotwireless|GetWirelessGateway|2xx->2xx|$.LoRaWAN.Beaconing
Issue-Key: iotwireless|GetWirelessGateway|2xx->2xx|$.Name
Issue-Key: iotwireless|GetWirelessGateway|4xx->2xx|$.Message
Issue-Key: iotwireless|GetWirelessGateway|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iotwireless|GetWirelessGateway|4xx->2xx|status
Issue-Commit: 9afa4a6b31ac525ec86093eb8c74984d268a84b3

# GetWirelessGateway drops Beaconing, ignores renames and deletion

> _Note: this issue was identified against the code base at commit `9afa4a6b31ac525ec86093eb8c74984d268a84b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWirelessGateway

## Description
Three distinct defects in the emulator's wireless-gateway read path and state, all on the iotwireless service. They share one root area: GetWirelessGateway does not render the full modelled LoRaWANGateway shape, and it reads a gateway record that neither mutations nor deletions are applied to. The same family of defect is already filed for other iotwireless resources (20261030T072116Z-iotwireless-getdeviceprofile-drops-lorawan-configuration-and-ignores-deletion.md and 20261030T115132Z-iotwireless-getmulticastgroup-drops-group-state-and-ignores-deletion.md); neither covers WirelessGateway, so this issue is net-new. 3 causes must be closed.

Cause 1 of 3: GetWirelessGateway omits LoRaWAN.Beaconing (fixture iot-wireless-wireless-gateway-lifecycle, step task#2).
AWS: Smithy: GetWirelessGateway output member LoRaWAN targets com.amazonaws.iotwireless#LoRaWANGateway, whose members include Beaconing -> com.amazonaws.iotwireless#Beaconing {DataRate: BeaconingDataRate @range 0..15, Frequencies: BeaconingFrequencies @length 0..10}. The recording shows AWS returning 200 with LoRaWAN.Beaconing present as a structure for a LoRaWAN gateway, with its own members unset. Recorded behavior only for the exact emptiness: AWS emits the Beaconing container for a LoRaWAN gateway even when no beaconing parameters were supplied at CreateWirelessGateway.
Emulator: returns 200 but the LoRaWAN object has no Beaconing key at all ($.LoRaWAN.Beaconing: missing, expected a Beaconing structure, got None).
Why: a client that reads response.LoRaWAN.Beaconing.DataRate / .Frequencies — or that round-trips a GetWirelessGateway response back into gateway configuration — sees the member absent and must special-case the emulator; SDKs that model Beaconing as always-present on LoRaWAN gateways lose the field, and code that diffs current vs desired beaconing state cannot tell 'unset' from 'not returned'.
Fix: the GetWirelessGateway response serializer / LoRaWANGateway projection in the iotwireless service crate must render the Beaconing member of LoRaWANGateway (with its DataRate and Frequencies members carried through from stored state, unset when never configured).
Traces: iot-wireless-wireless-gateway-lifecycle#task#2 (and again at #task#5).

Cause 2 of 3: UpdateWirelessGateway does not persist Name, so GetWirelessGateway keeps returning the pre-update name (fixture iot-wireless-wireless-gateway-lifecycle, step task#5).
AWS: Smithy: UpdateWirelessGateway PATCH /wireless-gateways/{Id}, HTTP 204, input members Name (WirelessGatewayName @length 0..256), Description, JoinEuiFilters, NetIdFilters, MaxEirp. A subsequent GetWirelessGateway returns 200 with Name equal to the value just supplied. The recording shows the renamed value observed on the following read.
Emulator: the update is accepted, but the next GetWirelessGateway returns the original creation-time name: $.Name value_mismatch — expected the renamed gateway name (the created name with the client's new suffix), got the unrenamed created name. The Name member is simply not written to state.
Why: rename is silently a no-op. Any client that updates a gateway and then reads it back to confirm (console flows, idempotent reconcilers, infrastructure-as-code drift detection) sees its write lost, concludes the update failed, and may retry forever or report permanent drift. Only Name is shown diverging by this step, but the same ignored-input path plausibly affects the other UpdateWirelessGateway members.
Fix: the UpdateWirelessGateway handler in the iotwireless service crate must apply the supplied Name (and the other optional input members) to the stored wireless-gateway record that GetWirelessGateway reads.
Traces: iot-wireless-wireless-gateway-lifecycle#task#5.

Cause 3 of 3: GetWirelessGateway on a deleted gateway returns 200 instead of 404 ResourceNotFoundException (fixture iot-wireless-wireless-gateway-lifecycle, step task#8).
AWS: Smithy: GetWirelessGateway declares error ResourceNotFoundException with @error client and @httpError 404 (doc: 'Resource does not exist'); DeleteWirelessGateway is DELETE /wireless-gateways/{Id} returning 204. After a successful delete, a read of the same identifier fails with HTTP 404, x-amzn-errortype naming ResourceNotFoundException, and body member Message. For this operation the recorded message is the fixed text "Wireless Gateway doesn't exist" — it has no variable part: no id, name or timestamp is interpolated into it (unlike the DeviceProfile/MulticastGroup form of this error, which also populates ResourceId and ResourceType).
Emulator: returns HTTP 200 with a normal gateway body — status: expected 404, got 200; $.Message missing (expected "Wireless Gateway doesn't exist", got None); $headers.x-amzn-errortype missing. The delete leaves the record readable, so the emulator serves a resource that no longer exists.
Why: deletion is unobservable. Clients that poll a get-until-404 to confirm teardown hang or time out; cleanup and test-isolation logic believes the gateway still exists and may try to delete it again or refuse to reuse its name; error-handling branches keyed on ResourceNotFoundException are never exercised.
Fix: the DeleteWirelessGateway handler / wireless-gateway state in the iotwireless service crate must remove the record (or mark it deleted), and the GetWirelessGateway handler must map a missing identifier to ResourceNotFoundException -> 404 with the x-amzn-errortype header and the Message above.
Traces: iot-wireless-wireless-gateway-lifecycle#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, 26/88 steps passed).

Cause 1 — fixture iot-wireless-wireless-gateway-lifecycle, step task#2, GetWirelessGateway, expected_status 200 / actual_status 200:
  $.LoRaWAN.Beaconing: missing (expected {'DataRate': None, 'Frequencies': None}, got None)

Cause 2 — fixture iot-wireless-wireless-gateway-lifecycle, step task#5, GetWirelessGateway, expected_status 200 / actual_status 200:
  $.LoRaWAN.Beaconing: missing (expected {'DataRate': None, 'Frequencies': None}, got None)
  $.Name: value_mismatch (expected the renamed gateway name '<created-name>-renamed', got the unrenamed created name '<created-name>')
  (the recording's literal values were 'sim2real-probe-iotw-gw-8782fdf0-renamed' vs 'sim2real-probe-iotw-gw-8782fdf0'; the generated name is per-run, the requirement is that the value supplied to UpdateWirelessGateway is what the next read returns)

Cause 3 — fixture iot-wireless-wireless-gateway-lifecycle, step task#8, GetWirelessGateway after DeleteWirelessGateway, expected_status 404 / actual_status 200:
  status: expected 404, got 200
  $.Message: missing (expected "Wireless Gateway doesn't exist", got None)
  $headers.x-amzn-errortype: missing (expected ResourceNotFoundException-shaped value, got None)

Model evidence (services/service-com-amazonaws-iotwireless-iotwireless/smithy-model/com-amazonaws-iotwireless-iotwireless.json):
  GetWirelessGateway: GET /wireless-gateways/{Identifier}, 200; output {Name, Id, Description, LoRaWAN: LoRaWANGateway, Arn, ThingName, ThingArn}; errors include ResourceNotFoundException (client, httpError 404).
  LoRaWANGateway members: GatewayEui, RfRegion, JoinEuiFilters, NetIdFilters, SubBands, Beaconing, MaxEirp.
  Beaconing members: DataRate (integer, @range 0..15), Frequencies (list, @length 0..10).
  UpdateWirelessGateway: PATCH /wireless-gateways/{Id}, 204; input {Id, Name @length 0..256, Description, JoinEuiFilters, NetIdFilters, MaxEirp}.
  DeleteWirelessGateway: DELETE /wireless-gateways/{Id}, 204.
  ResourceNotFoundException members: Message (@length 0..2048), ResourceId, ResourceType.

## Steps to Reproduce
Replay fixture iot-wireless-wireless-gateway-lifecycle against the emulator (replay_wire_corpus), or by hand against the iotwireless endpoint:

1. POST /wireless-gateways with a LoRaWAN gateway body (GatewayEui + RfRegion) to create a gateway; note the returned Id.
2. GET /wireless-gateways/{Id}?identifierType=WirelessGatewayId  ->  step task#2. Cause 1: the 200 body's LoRaWAN object has no Beaconing member; AWS returns the Beaconing structure.
3. PATCH /wireless-gateways/{Id} with {"Name": "<some-new-name>"}; expect 204.
4. GET /wireless-gateways/{Id}?identifierType=WirelessGatewayId  ->  step task#5. Cause 2: Name is still the creation-time name, not the value sent in step 3 (Cause 1 reproduces here too).
5. DELETE /wireless-gateways/{Id}; expect 204.
6. GET /wireless-gateways/{Id}?identifierType=WirelessGatewayId  ->  step task#8. Cause 3: the emulator answers 200 with the gateway body; AWS answers 404 with x-amzn-errortype ResourceNotFoundException and Message "Wireless Gateway doesn't exist".
