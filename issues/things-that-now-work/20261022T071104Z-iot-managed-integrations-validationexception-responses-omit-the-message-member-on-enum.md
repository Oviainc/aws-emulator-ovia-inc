Issue-Source: trace-sweep-divergence
Issue-Trace: iot-managed-integrations-managed-thing-error-paths
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-managed-integrations-managed-thing-error-paths iot-managed-integrations-provisioning-profile-lifecycle
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations|CreateManagedThing|4xx->4xx|$.Message
Issue-Key: iot-managed-integrations|CreateProvisioningProfile|4xx->4xx|$.Message
Issue-Commit: d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea

# ValidationException responses omit the Message member on enum validation

> _Note: this issue was identified against the code base at commit `d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateManagedThing

## Description
The emulator rejects invalid enum members with the correct HTTP 400 / ValidationException, but serializes the error body WITHOUT its `Message` member, so the body carries no explanation of what failed. Two operations show the same fault; both are one defect in how ValidationException is built/serialized for enum-constraint failures.

Cause 1 of 2: CreateManagedThing with an out-of-range AuthenticationMaterialType.
  AWS: Smithy: CreateManagedThing.AuthenticationMaterialType is @required, targets enum AuthMaterialType with values [CUSTOM_PROTOCOL_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE, ZWAVE_QR_BAR_CODE, ZIGBEE_QR_BAR_CODE, DISCOVERED_DEVICE, PRE_ONBOARDED_CLOUD]; modeled error ValidationException (errorKind client, httpError 400) has member Message -> ErrorMessage. A value outside that set is rejected with 400 ValidationException and a Message built FROM THE REQUEST naming the offending member and the constraint it failed (the recorded response shows the standard AWS shape: "1 validation error detected: Value at 'authenticationMaterialType' failed to satisfy constraint: Member must satisfy enum value set: [...]" — quoted as an example of that rule, not as required text).
  Emulator: returns the right status (400) and error shape, but `$.Message` is absent from the body entirely (replay detail: "$.Message: missing ... got None").
  Why: an SDK client reads the member only as `e.message()`; with no Message the exception surfaces empty, so callers and logs cannot tell WHICH member was invalid and retry/repair logic that inspects the message gets nothing. An absent modeled member is also a shape divergence, not just prose.
  Fix: CreateManagedThing's input validation / ValidationException construction — populate Message from the member name plus the violated enum constraint when rejecting.
  Traces: iot-managed-integrations-managed-thing-error-paths#task#4.

Cause 2 of 2: CreateProvisioningProfile with an out-of-range ProvisioningType.
  AWS: Smithy: CreateProvisioningProfile.ProvisioningType is @required, targets enum ProvisioningType with values [FLEET_PROVISIONING, JITR]; same modeled ValidationException (400) with a Message member. An invalid value yields 400 with a request-derived Message naming `provisioningType` and the permitted enum set (recorded example: "1 validation error detected: Value at 'provisioningType' failed to satisfy constraint: Member must satisfy enum value set: [JITR, FLEET_PROVISIONING]").
  Emulator: 400 is correct, `$.Message` is missing from the body (replay detail: "$.Message: missing ... got None").
  Why: identical client impact — an empty error message on a create call that a provisioning workflow must diagnose.
  Fix: CreateProvisioningProfile's input validation / shared ValidationException serializer for this service.
  Traces: iot-managed-integrations-provisioning-profile-lifecycle#task#3.

## Evidence
Cause 1 — fixture iot-managed-integrations-managed-thing-error-paths step task#4, CreateManagedThing: expected 400 with $.Message describing the failed `authenticationMaterialType` enum constraint; actual 400 with $.Message missing (None).
Cause 2 — fixture iot-managed-integrations-provisioning-profile-lifecycle step task#3, CreateProvisioningProfile: expected 400 with $.Message describing the failed `provisioningType` enum constraint; actual 400 with $.Message missing (None).
Status and error classification match in both; the modeled ValidationException.Message member is simply never emitted. Model: ValidationException {Message: ErrorMessage}, httpError 400; AuthMaterialType / ProvisioningType enum value sets as listed above.

## Steps to Reproduce
1. Replay fixture iot-managed-integrations-managed-thing-error-paths against the emulator and inspect step task#4: POST /managed-things with AuthenticationMaterialType set to a value outside AuthMaterialType. Observe 400 whose JSON body has no Message key.
2. Replay fixture iot-managed-integrations-provisioning-profile-lifecycle step task#3: POST /provisioning-profiles with ProvisioningType set to a value outside [FLEET_PROVISIONING, JITR]. Observe 400 whose JSON body has no Message key.
3. Both should carry a Message naming the rejected member and the enum value set it must satisfy.
