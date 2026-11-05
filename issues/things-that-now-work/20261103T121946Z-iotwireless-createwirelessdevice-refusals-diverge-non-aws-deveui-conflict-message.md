Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotwireless.duplicate-dev-eui-rejected-within-account iotwireless.wireless-device-mac-version-must-match-device-profile
Issue-Shape-Id: com.amazonaws.iotwireless#iotwireless
Issue-Key: iotwireless.duplicate-dev-eui-rejected-within-account|run#22|AssertionError|assert message == "Device with given DevEui already exists", f"unexpected message {message!r}"
Issue-Key: iotwireless.wireless-device-mac-version-must-match-device-profile|_error#20|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: 96d474d008ab76fb45cae77789f6f9a19e9776a3

# CreateWirelessDevice refusals diverge: non-AWS DevEui conflict message, and no MacVersion/OTAA-version cross-check at all

> _Note: this issue was identified against the code base at commit `96d474d008ab76fb45cae77789f6f9a19e9776a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWirelessDevice

## Description
Two net-new root causes, both in the CreateWirelessDevice refusal path of the iotwireless crate. Both are about how the operation refuses a bad create: one refuses with the emulator's own prose instead of AWS's sentence, the other does not refuse at all. Neither is covered by the open issue 20261029T171855Z-iotwireless-validation-refusals-carry-no-message-and-wrong.md (that one is about the generic '1 validation error detected: ...' constraint template on CreateDestination/CreateDeviceProfile/CreateNetworkAnalyzerConfiguration/CreateWirelessGateway/CreateMulticastGroup; these two are CreateWirelessDevice semantic refusals with their own fixed sentences).

Cause 1 of 2: ConflictException message for a DevEui already claimed in the account.
AWS: a second CreateWirelessDevice whose LoRaWAN.DevEui is already registered in the account is refused 409 ConflictException (Smithy: CreateWirelessDevice errors include ConflictException @httpError 409; ConflictException members Message/ResourceId/ResourceType, wire key `Message`). The message is a FIXED sentence with no variable parts at all: "Device with given DevEui already exists". It names neither the EUI nor the owning device id.
Emulator: the status and code are right (ConflictException), but the message is the crate's own templated prose naming the request's EUI and the existing device's id, e.g. "DevEui fd585ec00ceae499 is already registered to wireless device 1ba35dba-7573-45ed-a834-60c9e07d2e2d" (the hex EUI comes from the request and the UUID from emulator state; both are per-run values, the defect is the template, not these values).
Why: a client or test that branches on the refusal text — the only place AWS distinguishes a duplicate-DevEui conflict from the idempotency-token conflict the same operation also raises with code ConflictException — sees an unrecognized sentence and cannot classify the conflict; worse, the emulator's wording leaks another device's id to a caller AWS tells nothing about.
Fix: the duplicate-DevEui refusal in CreateWirelessDevice's handler must emit AWS's fixed ConflictException message text (see Traces for the exact site).
Traces: check iotwireless.duplicate-dev-eui-rejected-within-account; site services/service-com-amazonaws-iotwireless-iotwireless/src/ops/create_wireless_device.rs:257-270.

Cause 2 of 2: OTAA key-material version is never cross-checked against the referenced device profile's MacVersion, so a mismatched create succeeds.
AWS: CreateWirelessDevice with LoRaWAN.OtaaV1_1 key material (AppKey/NwkKey/JoinEui — LoRaWAN 1.1 provisioning) against a LoRaWAN.DeviceProfileId whose profile was created with LoRaWAN.MacVersion "1.0.3" is refused 400 ValidationException (Smithy: CreateWirelessDevice errors include ValidationException @httpError 400; ValidationException$Message, wire key `Message`), and no device is created — the rejected DevEui does not appear in ListWirelessDevices. Message = "Validation error: the input profile version is not match macVersion defines in deviceProfile: <macVersion>": the sentence (including its ungrammatical "is not match") is fixed text; the only variable part is the trailing MacVersion value read from the referenced device profile in state ("1.0.3" in the recording). The same request against a profile whose MacVersion is "1.1" is accepted and GetWirelessDevice reports that DeviceProfileId. This refusal is recorded behavior only — the model declares no structural link between the OtaaV1_1/OtaaV1_0_x/AbpV1_1/AbpV1_0_x members of LoRaWANDevice and the profile's MacVersion.
Emulator: the mismatched create SUCCEEDS with HTTP 201 and returns an Arn/Id (e.g. Id 71072f00-12e2-4867-ad5a-717d6fcf69a7), i.e. the device is registered against a profile whose MAC version cannot provision it. Reading the handler, the only validations performed are Type/Positioning enum membership, destination existence, DevEui uniqueness and a store-size ceiling; the referenced DeviceProfileId is not resolved at all, let alone compared against the supplied OTAA/ABP version.
Why: a client that relies on AWS rejecting incoherent provisioning gets a device that real IoT Wireless would never have created, so its error path is never exercised and its state diverges from AWS's; code that treats 201 as proof the profile and key material agree then mis-provisions.
Fix: CreateWirelessDevice's input validation — resolve LoRaWAN.DeviceProfileId in the service state, compare the profile's LoRaWAN.MacVersion against the OTAA/ABP version member supplied on the request, and refuse with the modeled ValidationException (message as above) before any device is inserted.
Traces: check iotwireless.wireless-device-mac-version-must-match-device-profile; handler services/service-com-amazonaws-iotwireless-iotwireless/src/ops/create_wireless_device.rs:177-200 (the only validation present) and :288-319 (the unconditional insert).

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout. iotwireless scored 0/8; six of the eight failures are already-filed unimplemented-stub blocks, the two below are net-new and both land in CreateWirelessDevice.

1) [mismatch] iotwireless.duplicate-dev-eui-rejected-within-account
   The check creates a LoRaWAN device with a DevEui, then creates a second device reusing that DevEui. Code assertion passes (ConflictException); the message assertion fails:
     assert message == "Device with given DevEui already exists"
     AssertionError: unexpected message 'DevEui fd585ec00ceae499 is already registered to wireless device 1ba35dba-7573-45ed-a834-60c9e07d2e2d'
   Expected: 409 ConflictException, Message exactly "Device with given DevEui already exists" (wholly fixed text).
   Actual: 409 ConflictException with the emulator-authored sentence "DevEui {dev_eui} is already registered to wireless device {owner}".
   Source: services/service-com-amazonaws-iotwireless-iotwireless/src/ops/create_wireless_device.rs:261-263 formats exactly that string.

2) [mismatch] iotwireless.wireless-device-mac-version-must-match-device-profile
   The check creates a device profile with LoRaWAN.MacVersion "1.0.3", a second with "1.1", a service profile and a destination, then calls CreateWirelessDevice(Type="LoRaWAN", DestinationName=dest, LoRaWAN={DeviceProfileId: <1.0.3 profile>, ServiceProfileId: ..., DevEui: ..., OtaaV1_1: {AppKey, NwkKey, JoinEui}}) and expects a refusal:
     AssertionError: expected the call to be refused, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'Arn': 'arn:aws:iotwireless:us-west-2:000000000000:WirelessDevice/71072f00-12e2-4867-ad5a-717d6fcf69a7', 'Id': '71072f00-12e2-4867-ad5a-717d6fcf69a7'}
   Expected: 400 ValidationException, Message "Validation error: the input profile version is not match macVersion defines in deviceProfile: 1.0.3" (the trailing version is the referenced profile's MacVersion from state), and the device absent from ListWirelessDevices.
   Actual: 201 Created with a minted Arn/Id; the device is persisted.

Model evidence (services/service-com-amazonaws-iotwireless-iotwireless/smithy-model/com-amazonaws-iotwireless-iotwireless.json): CreateWirelessDevice POST /wireless-devices, success 201, errors AccessDenied(403)/Conflict(409)/InternalServer(500)/ResourceNotFound(404)/Throttling(429)/Validation(400); ConflictException{Message,ResourceId,ResourceType}; ValidationException{Message}; LoRaWANDevice{DevEui ^[a-fA-F0-9]{16}$, DeviceProfileId, ServiceProfileId, OtaaV1_1, OtaaV1_0_x, AbpV1_1, AbpV1_0_x, FPorts}.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay suite simparity-behavioral-aws-v1 (service iotwireless).
2. Cause 1 — replay check iotwireless.duplicate-dev-eui-rejected-within-account: CreateWirelessDevice(Type="LoRaWAN", DestinationName=<existing destination>, LoRaWAN={DeviceProfileId, ServiceProfileId, DevEui: X, OtaaV1_0_x: {...}}), then repeat with the same DevEui X and a different name. Read the 409 body's Message: it is the emulator's "DevEui X is already registered to wireless device <id>" instead of AWS's fixed "Device with given DevEui already exists".
3. Cause 2 — replay check iotwireless.wireless-device-mac-version-must-match-device-profile: CreateDeviceProfile(LoRaWAN={MacVersion: "1.0.3"}), CreateServiceProfile(LoRaWAN={}), CreateDestination(...), then CreateWirelessDevice(Type="LoRaWAN", DestinationName=<that destination>, LoRaWAN={DeviceProfileId: <the 1.0.3 profile>, ServiceProfileId: ..., DevEui: ..., OtaaV1_1: {AppKey, NwkKey, JoinEui}}). The call returns 201 instead of a 400 ValidationException, and ListWirelessDevices then shows the device that AWS would never have created. Repeating against a profile with MacVersion "1.1" must still succeed.
