Issue-Source: trace-sweep-divergence
Issue-Trace: iot-wireless-network-analyzer-configuration-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-wireless-network-analyzer-configuration-lifecycle
Issue-Shape-Id: com.amazonaws.iotwireless#iotwireless
Issue-Key: iotwireless|GetNetworkAnalyzerConfiguration|2xx->2xx|$.Description
Issue-Key: iotwireless|GetNetworkAnalyzerConfiguration|2xx->2xx|$.TraceContent.MulticastFrameInfo
Issue-Key: iotwireless|GetNetworkAnalyzerConfiguration|4xx->2xx|$.Message
Issue-Key: iotwireless|GetNetworkAnalyzerConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iotwireless|GetNetworkAnalyzerConfiguration|4xx->2xx|status
Issue-Commit: e8d34fd8b87a640bf972b7f1f05015aa932d31e1

# iotwireless NetworkAnalyzerConfiguration read path drops TraceContent/Description and survives deletion

> _Note: this issue was identified against the code base at commit `e8d34fd8b87a640bf972b7f1f05015aa932d31e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetNetworkAnalyzerConfiguration

## Description
GetNetworkAnalyzerConfiguration in the iotwireless emulator returns an under-populated body and keeps answering 200 for a configuration that has been deleted. Three causes, all in the NetworkAnalyzerConfiguration read path and its state.

Cause 1 of 3 — GetNetworkAnalyzerConfiguration omits Description and TraceContent.MulticastFrameInfo right after create (fixture iot-wireless-network-analyzer-configuration-lifecycle, step task#2).
AWS: Smithy: GetNetworkAnalyzerConfiguration output members TraceContent (structure com.amazonaws.iotwireless#TraceContent with members WirelessDeviceFrameInfo, LogLevel, MulticastFrameInfo — all enums) and Description (@length min 0 max 2048). AWS populates the full trace-content triple for a configuration created with CreateNetworkAnalyzerConfiguration, defaulting the frame-info members that the request did not set, and echoes Description even when it is the empty string. Recorded behavior: the response carries Description as an empty string and TraceContent.MulticastFrameInfo = DISABLED (the service-side default for a member the create request left unset).
Emulator: both keys are absent from the JSON body — Description: missing (expected the empty string, got None) and TraceContent.MulticastFrameInfo: missing (expected 'DISABLED', got None) — while status is the correct 200.
Why: a client reading the configuration cannot see the multicast frame-info setting at all, so a round-trip (get, modify, update) silently loses it, and code that reads Description unconditionally gets a null where AWS always supplies a string. A configuration that AWS reports as fully specified looks half-configured.
Fix: the GetNetworkAnalyzerConfiguration handler's output serializer plus the create-time state defaults for TraceContent — unset frame-info members must be stored at their service default rather than left absent, and Description must be serialized when empty rather than skipped.
Traces: iot-wireless-network-analyzer-configuration-lifecycle#task#2.

Cause 2 of 3 — TraceContent.MulticastFrameInfo is still absent after UpdateNetworkAnalyzerConfiguration (fixture iot-wireless-network-analyzer-configuration-lifecycle, step task#5).
AWS: Smithy: UpdateNetworkAnalyzerConfiguration (PATCH /network-analyzer-configurations/{ConfigurationName}, 204) accepts TraceContent and Description; a subsequent GetNetworkAnalyzerConfiguration returns the merged configuration, including every TraceContent member — those the update changed and those it left alone. Recorded behavior: the get after the update still reports MulticastFrameInfo = DISABLED, i.e. an update that does not mention a frame-info member leaves it at its stored value rather than erasing it.
Emulator: after the update the get again omits the key entirely — TraceContent.MulticastFrameInfo: missing (expected 'DISABLED', got None), status 200 as expected and Description now present.
Why: the member is unreadable across the whole lifecycle, not just at create, so a client can never confirm a multicast-frame-info setting it applied and an update that is meant to preserve it is indistinguishable from one that cleared it.
Fix: the UpdateNetworkAnalyzerConfiguration handler's merge into the configuration's stored state, together with the same GetNetworkAnalyzerConfiguration serializer as cause 1 — the stored TraceContent must retain unmentioned members and the read must emit all three.
Traces: iot-wireless-network-analyzer-configuration-lifecycle#task#5.

Cause 3 of 3 — GetNetworkAnalyzerConfiguration returns 200 for a deleted configuration instead of 404 ResourceNotFoundException (fixture iot-wireless-network-analyzer-configuration-lifecycle, step task#8).
AWS: Smithy: GetNetworkAnalyzerConfiguration models ResourceNotFoundException (errorKind client, httpError 404, members Message/ResourceId/ResourceType) and DeleteNetworkAnalyzerConfiguration (DELETE /network-analyzer-configurations/{ConfigurationName}, 204) removes the resource. A get on a name that no longer exists is refused with HTTP 404, the x-amzn-errortype header naming ResourceNotFoundException, and the body's Message. For this error class AWS words the message as the fixed text "Configuration " + " is not exist" wrapped around one variable part, the ConfigurationName taken from the request path — in the recording that variable part was the probe's configuration name, which is a per-run value and not part of the requirement.
Emulator: the get after a successful delete answers HTTP 200 with the configuration body — status: expected 404, got 200; $.Message missing (expected the "Configuration <ConfigurationName> is not exist" text, got None); $headers.x-amzn-errortype missing entirely.
Why: deletion is not observable. A client that deletes a configuration and polls for its disappearance never sees the 404 it waits for, so teardown loops hang or report the resource as live; error handling keyed on ResourceNotFoundException / the x-amzn-errortype header never fires, and a recreate-after-delete flow can collide with a ghost record.
Fix: DeleteNetworkAnalyzerConfiguration's mutation of the configuration state (the record must actually be removed, or tombstoned) and the GetNetworkAnalyzerConfiguration handler's not-found branch, which must raise the modeled ResourceNotFoundException so the shared error serializer emits status 404, the x-amzn-errortype header and the Message.
Traces: iot-wireless-network-analyzer-configuration-lifecycle#task#8.

Note: the sibling iotwireless validation-message issue 20261029T171855Z-iotwireless-validation-refusals-carry-no-message-and-wrong.md owns the CreateNetworkAnalyzerConfiguration traceContent.logLevel enum-validation message (step task#6 of the same fixture); it does not cover any of the three causes above.

## Evidence
Replay of fixture iot-wireless-network-analyzer-configuration-lifecycle (trace set simparity-wire-aws-v1), unowned clusters:

1) step task#2, GetNetworkAnalyzerConfiguration, expected 200 / actual 200:
   $.Description: missing (expected '', got None)
   $.TraceContent.MulticastFrameInfo: missing (expected 'DISABLED', got None)

2) step task#5, GetNetworkAnalyzerConfiguration (after UpdateNetworkAnalyzerConfiguration), expected 200 / actual 200:
   $.TraceContent.MulticastFrameInfo: missing (expected 'DISABLED', got None)

3) step task#8, GetNetworkAnalyzerConfiguration (after DeleteNetworkAnalyzerConfiguration), expected 404 / actual 200:
   status: expected 404, got 200
   $.Message: missing (expected 'Configuration <ConfigurationName> is not exist', got None)
   $headers.x-amzn-errortype: missing (expected the ResourceNotFoundException shape id, got None)

Model evidence: Smithy GetNetworkAnalyzerConfiguration output = {TraceContent, WirelessDevices, WirelessGateways, Description, Arn, Name, MulticastGroups}; errors include ResourceNotFoundException (client, httpError 404) with members Message (@length 0..2048), ResourceId, ResourceType. TraceContent = {WirelessDeviceFrameInfo, LogLevel, MulticastFrameInfo}, all enums. DeleteNetworkAnalyzerConfiguration = DELETE /network-analyzer-configurations/{ConfigurationName}, success 204.

## Steps to Reproduce
Replay fixture iot-wireless-network-analyzer-configuration-lifecycle from trace set simparity-wire-aws-v1 against the emulator, or by hand against the iotwireless endpoint:
1. POST /network-analyzer-configurations with Name=<config-name> and a TraceContent that sets WirelessDeviceFrameInfo and LogLevel but not MulticastFrameInfo (201).
2. GET /network-analyzer-configurations/<config-name> -> body omits Description and TraceContent.MulticastFrameInfo; AWS returns Description (empty string) and MulticastFrameInfo=DISABLED. (cause 1, step task#2)
3. PATCH /network-analyzer-configurations/<config-name> with a Description change (204), then GET again -> TraceContent.MulticastFrameInfo is still absent. (cause 2, step task#5)
4. DELETE /network-analyzer-configurations/<config-name> (204), then GET /network-analyzer-configurations/<config-name> -> emulator answers 200 with the configuration body; AWS answers 404 with x-amzn-errortype: ResourceNotFoundException and Message "Configuration <config-name> is not exist". (cause 3, step task#8)
