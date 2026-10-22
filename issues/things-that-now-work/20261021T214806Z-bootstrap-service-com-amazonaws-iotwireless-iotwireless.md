Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the IoT Wireless (iotwireless) emulator

_Filed: 2026-10-21T21:48:06Z_
_Operation: service-com-amazonaws-iotwireless-iotwireless | Class: stub_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 112 operations in `services/service-com-amazonaws-iotwireless-iotwireless/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, and the service holds no state. Implement a FIRST, SMALL stateful slice: the destination + wireless-device lifecycle, which is the core loop every IoT Wireless client starts with (a destination routes device uplinks; a wireless device is registered against that destination).

Happy path to implement, in order (protocol: `restJson1`, signing name `iotwireless`):

1. **CreateDestination** — `POST /destinations` -> 201. Input: `Name` (required, len 0..128, `^[a-zA-Z0-9-_]+$`), `ExpressionType` (required, `MqttTopic|RuleName`), `Expression` (required, len 0..2048), `RoleArn` (required, len 20..2048), optional `Description`, `Tags`, `ClientRequestToken`. Output: `Arn`, `Name`. Must persist the destination under its name and mint a stable ARN of the form `arn:aws:iotwireless:<region>:<account>:Destination/<Name>`. Creating a destination whose `Name` already exists must fail with `ConflictException` (409), not silently overwrite.
2. **GetDestination** — `GET /destinations/{Name}` -> 200. Output `Arn`, `Name`, `Expression`, `ExpressionType`, `Description`, `RoleArn` read back from the stored record (byte-identical to what CreateDestination stored). Unknown name -> `ResourceNotFoundException` (404).
3. **CreateWirelessDevice** — `POST /wireless-devices` -> 201. Input: `Type` (required, `LoRaWAN|Sidewalk`), `DestinationName` (required), optional `Name`, `Description`, `LoRaWAN`, `Positioning`, `Tags`, `ClientRequestToken`. Output: `Arn`, `Id`. Must mint a fresh device `Id` (uuid), persist the whole record (including `LoRaWAN` DevEui/AppKey info as supplied), and reject a `DestinationName` that does not exist in the destination store with `ResourceNotFoundException` (404) — the destination link must be checked against real state, not assumed.
4. **GetWirelessDevice** — `GET /wireless-devices/{Identifier}?identifierType=...` -> 200. `IdentifierType` is required (`WirelessDeviceId|DevEui|ThingName|SidewalkManufacturingSn`); at minimum `WirelessDeviceId` and `DevEui` lookups must resolve from the stored devices. Output `Id`, `Arn`, `Type`, `Name`, `Description`, `DestinationName`, `LoRaWAN`, `Positioning` must all come from the persisted record created in step 3. Unknown identifier -> `ResourceNotFoundException` (404).
5. **ListWirelessDevices** — `GET /wireless-devices` -> 200. Output `WirelessDeviceList` (list of `WirelessDeviceStatistics`: `Arn`, `Id`, `Type`, `Name`, `DestinationName`, `LoRaWAN`, `Positioning`) plus `NextToken`. Must enumerate the real device store, honour `MaxResults` (0..250) by truncating and returning an opaque `NextToken` that resumes exactly where the previous page stopped, and honour at least the `DestinationName` and `WirelessDeviceType` query filters.
6. **DeleteWirelessDevice** — `DELETE /wireless-devices/{Id}` -> 204, then **DeleteDestination** — `DELETE /destinations/{Name}` -> 204. Both must remove the record so the subsequent Get returns `ResourceNotFoundException` (404) and the List no longer includes it. Deleting an id/name that does not exist -> 404.

Every one of these must compute its response from persisted state held in the shared engine store reached via `crate::state::service_state(&*self.engine)`. No canned bodies, no echoing the input back, no minting an id that is never stored — a create followed by a get/list must return exactly what was created, and a delete must be observable by the next caller.

## State Model

In `src/state.rs` (`ComAmazonawsIotwirelessIotwirelessState`):

- `destinations: BTreeMap<String /*Name*/, Destination>` where `Destination { name: String, arn: String, expression: String, expression_type: ExpressionType, description: Option<String>, role_arn: String, tags: Vec<Tag> }`.
- `wireless_devices: BTreeMap<String /*Id*/, WirelessDevice>` where `WirelessDevice { id: String /*uuid*/, arn: String, device_type: WirelessDeviceType, name: Option<String>, description: Option<String>, destination_name: String, lorawan: Option<LoRaWANDevice>, sidewalk: Option<SidewalkDevice>, positioning: Option<PositioningConfigStatus>, thing_name: Option<String>, thing_arn: Option<String>, tags: Vec<Tag>, created_at: SystemTime, last_uplink_received_at: Option<String> }`.
- `dev_eui_index: BTreeMap<String /*DevEui*/, String /*device id*/>` so `GetWirelessDevice` can resolve `identifierType=DevEui` without scanning.
- Insertion order / sorted-key iteration over `wireless_devices` provides the stable ordering that `ListWirelessDevices` pagination tokens index into (token = opaque encoding of the last returned key).

All other operations (gateways, FUOTA tasks, multicast groups, profiles, positioning, log levels, metrics, import tasks, tagging, event configuration) stay 501 for now and will be filed as individual gap issues later.

## Evidence

Spec: `services/service-com-amazonaws-iotwireless-iotwireless/smithy-model/com-amazonaws-iotwireless-iotwireless.json` and the generated `OPERATIONS.md` in the same crate — `CreateDestination` (OPERATIONS.md:82-87, `POST /destinations` -> 201, required Name/ExpressionType/Expression/RoleArn, output Arn+Name), `GetDestination` (:292-297, `GET /destinations/{Name}`, returns the stored Expression/ExpressionType/Description/RoleArn), `CreateWirelessDevice` (:124-129, required Type + DestinationName, output Arn+Id, declares ResourceNotFoundException), `GetWirelessDevice` (:427-432, `GET /wireless-devices/{Identifier}` with required `identifierType` query, returns the full device record), `ListWirelessDevices` (:597-603, @paginated inputToken=NextToken/outputToken=NextToken, MaxResults 0..250, DestinationName/WirelessDeviceType filters, output WirelessDeviceList of WirelessDeviceStatistics), `DeleteWirelessDevice` (:201-206, 204) and `DeleteDestination` (:152-157, 204). `WirelessDeviceStatistics`/`Destinations` member lists confirmed via smithy_query. Implementation state: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-iotwireless-iotwireless/src/ops/` matches every handler file, including `create_destination.rs`, `get_destination.rs`, `create_wireless_device.rs`, `get_wireless_device.rs`, `list_wireless_devices.rs`, `delete_wireless_device.rs` and `delete_destination.rs`; the crate has no tests beyond the generated `tests/state_roundtrip.rs`, so nothing asserts any of this behavior today.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI iotwireless client at it.
2. `aws iotwireless create-destination --name MyDest --expression-type RuleName --expression MyRule --role-arn arn:aws:iam::123456789012:role/iotwireless` — expect 201 with Arn + Name; today returns HTTP 501 NotImplemented.
3. `aws iotwireless get-destination --name MyDest` — expect 200 echoing the stored Expression=MyRule, ExpressionType=RuleName, RoleArn and the same Arn.
4. `aws iotwireless create-wireless-device --type LoRaWAN --name dev1 --destination-name MyDest --lo-ra-wan '{"DevEui":"0000000000000001","DeviceProfileId":"dp"}'` — expect 201 with a fresh Id and Arn.
5. `aws iotwireless create-wireless-device --type LoRaWAN --destination-name NoSuchDest` — expect ResourceNotFoundException (404), not success.
6. `aws iotwireless get-wireless-device --identifier <Id> --identifier-type WirelessDeviceId` — expect 200 with Name=dev1, DestinationName=MyDest, Type=LoRaWAN and the stored LoRaWAN block.
7. `aws iotwireless list-wireless-devices --max-results 1` — expect exactly one WirelessDeviceStatistics entry plus a NextToken; following the NextToken returns the remaining devices with no repeats or omissions; `--destination-name MyDest` filters to devices on that destination.
8. `aws iotwireless delete-wireless-device --id <Id>` — expect 204; then get-wireless-device on the same Id returns ResourceNotFoundException (404) and list-wireless-devices no longer contains it.
9. `aws iotwireless delete-destination --name MyDest` — expect 204; then get-destination returns ResourceNotFoundException (404).
