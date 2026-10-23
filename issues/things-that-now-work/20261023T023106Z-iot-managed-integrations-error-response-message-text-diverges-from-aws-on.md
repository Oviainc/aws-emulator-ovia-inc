Issue-Source: trace-sweep-divergence
Issue-Trace: iot-managed-integrations-managed-thing-error-paths
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-managed-integrations-managed-thing-error-paths iot-managed-integrations-provisioning-profile-lifecycle
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations|UpdateManagedThing|4xx->4xx|$.Message
Issue-Key: iot-managed-integrations|DeleteManagedThing|4xx->4xx|$.Message
Issue-Key: iot-managed-integrations|CreateManagedThing|4xx->4xx|$.Message
Issue-Key: iot-managed-integrations|CreateProvisioningProfile|4xx->4xx|$.Message
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# Error response Message text diverges from AWS on iot-managed-integrations error paths

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateManagedThing

## Description
This bundle has FOUR causes. In every one the HTTP status and the error shape are already correct; what diverges is the `Message` member the emulator renders into the error body, and in each case the text is produced by a rule that is known (it is built from the request member being rejected), so the text is part of the contract and not just an example. A fixer must close all four before submitting.

Cause 1 of 4 - UpdateManagedThing, 404 ResourceNotFoundException message template.
AWS: Smithy: UpdateManagedThing PUT /managed-things/{Identifier}, error ResourceNotFoundException (httpError 404, doc 'The specified resource does not exist.'), member Message -> ErrorMessage. The model does not fix the text, but the recorded 404 body builds it from the request's `Identifier` path label, i.e. the rule is "ManagedThing for identifier '<Identifier>' not found" (recorded example for Identifier=nonexistentmanagedthing0001: "ManagedThing for identifier 'nonexistentmanagedthing0001' not found").
Emulator: returns 404 with Message built from a different, service-wide template - 'Managed thing nonexistentmanagedthing0001 was not found' (replay detail: $.Message value_mismatch).
Why: a client or test that asserts on the not-found reason text for a mutation on a missing managed thing sees a message in a shape AWS never emits; the request-derived template is the only part of the message AWS guarantees, and the emulator's phrasing is not it.
Fix: the UpdateManagedThing handler's not-found path (the ResourceNotFoundException it constructs) must format Message from the requested Identifier using AWS's template rather than the shared generic one.
Traces: iot-managed-integrations-managed-thing-error-paths#task#2.

Cause 2 of 4 - DeleteManagedThing, same 404 message template.
AWS: Smithy: DeleteManagedThing DELETE /managed-things/{Identifier}, error ResourceNotFoundException (httpError 404). The recorded 404 body uses the same request-derived rule as Cause 1, "ManagedThing for identifier '<Identifier>' not found".
Emulator: returns 404 with 'Managed thing <id> was not found' instead.
Why: same as Cause 1 - the delete-of-missing-resource path reports a reason text AWS does not produce, so error-text assertions and log scraping diverge.
Fix: the DeleteManagedThing handler's not-found path; it should share the same AWS-shaped formatter introduced for Cause 1 rather than the generic one.
Traces: iot-managed-integrations-managed-thing-error-paths#task#3.

Cause 3 of 4 - CreateManagedThing, ValidationException enum value set is listed in the wrong order.
AWS: Smithy: CreateManagedThing.AuthenticationMaterialType is required and targets the enum AuthMaterialType, whose declared values are CUSTOM_PROTOCOL_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE, ZWAVE_QR_BAR_CODE, ZIGBEE_QR_BAR_CODE, DISCOVERED_DEVICE, PRE_ONBOARDED_CLOUD; error ValidationException (httpError 400). The rejection message is built from the request by the standard AWS validation framework and names the member and its constraint: "1 validation error detected: Value at 'authenticationMaterialType' failed to satisfy constraint: Member must satisfy enum value set: [...]". The recorded response enumerates the set in the REVERSE of model declaration order: [PRE_ONBOARDED_CLOUD, ZIGBEE_QR_BAR_CODE, ZWAVE_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE, DISCOVERED_DEVICE, CUSTOM_PROTOCOL_QR_BAR_CODE].
Emulator: returns the correct 400/ValidationException and the correct message skeleton, but enumerates the set in model declaration order: [CUSTOM_PROTOCOL_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE, ZWAVE_QR_BAR_CODE, ZIGBEE_QR_BAR_CODE, DISCOVERED_DEVICE, PRE_ONBOARDED_CLOUD].
Why: the whole value of this message is that it tells a caller which values are acceptable, and clients and fixtures compare it as a whole string; the ordering is part of the generated text, so every such rejection differs from AWS byte-for-byte even though the constraint reported is the same.
Fix: the shared enum-constraint validation message builder (the validator that renders 'Member must satisfy enum value set'), which must emit the enum members in the order AWS does - the reverse of the Smithy declaration order - for every enum-typed member, not just this one.
Traces: iot-managed-integrations-managed-thing-error-paths#task#4.

Cause 4 of 4 - CreateProvisioningProfile, same enum ordering fault on a second enum.
AWS: Smithy: CreateProvisioningProfile.ProvisioningType is required and targets enum ProvisioningType with declared values FLEET_PROVISIONING, JITR; error ValidationException (httpError 400). The recorded rejection again lists the set reversed: "... Value at 'provisioningType' failed to satisfy constraint: Member must satisfy enum value set: [JITR, FLEET_PROVISIONING]".
Emulator: emits [FLEET_PROVISIONING, JITR] - declaration order again.
Why: confirms the fault is in the shared validation-message builder rather than in one operation, so it affects every enum-typed member of this service; a caller comparing the rejection text never matches AWS.
Fix: same shared enum-constraint message builder as Cause 3; this cause exists to pin that the fix must be generic (two different enums, two different operations).
Traces: iot-managed-integrations-provisioning-profile-lifecycle#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200, bootstrap_signal false).

Cause 1 - fixture iot-managed-integrations-managed-thing-error-paths step task#2, UpdateManagedThing: expected status 404, actual 404; $.Message expected "ManagedThing for identifier 'nonexistentmanagedthing0001' not found", actual 'Managed thing nonexistentmanagedthing0001 was not found'.

Cause 2 - same fixture, step task#3, DeleteManagedThing: expected status 404, actual 404; $.Message expected "ManagedThing for identifier 'nonexistentmanagedthing0001' not found", actual 'Managed thing nonexistentmanagedthing0001 was not found'.

Cause 3 - same fixture, step task#4, CreateManagedThing: expected status 400, actual 400; $.Message expected "1 validation error detected: Value at 'authenticationMaterialType' failed to satisfy constraint: Member must satisfy enum value set: [PRE_ONBOARDED_CLOUD, ZIGBEE_QR_BAR_CODE, ZWAVE_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE, DISCOVERED_DEVICE, CUSTOM_PROTOCOL_QR_BAR_CODE]", actual "1 validation error detected: Value at 'authenticationMaterialType' failed to satisfy constraint: Member must satisfy enum value set: [CUSTOM_PROTOCOL_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE, ZWAVE_QR_BAR_CODE, ZIGBEE_QR_BAR_CODE, DISCOVERED_DEVICE, PRE_ONBOARDED_CLOUD]" - same member, same constraint, reversed value list.

Cause 4 - fixture iot-managed-integrations-provisioning-profile-lifecycle step task#3, CreateProvisioningProfile: expected status 400, actual 400; $.Message expected "1 validation error detected: Value at 'provisioningType' failed to satisfy constraint: Member must satisfy enum value set: [JITR, FLEET_PROVISIONING]", actual "... enum value set: [FLEET_PROVISIONING, JITR]".

Model references used: Smithy AuthMaterialType values in declaration order are [CUSTOM_PROTOCOL_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE, ZWAVE_QR_BAR_CODE, ZIGBEE_QR_BAR_CODE, DISCOVERED_DEVICE, PRE_ONBOARDED_CLOUD]; ProvisioningType values are [FLEET_PROVISIONING, JITR]; ResourceNotFoundException httpError 404 with member Message; ValidationException httpError 400 with member Message. The emulator reproduces declaration order, AWS reproduces its reverse in both recordings.

Related but separate, already open: 20261022T145600Z-iot-managed-integrations-updatemanagedthing-204-responses-omit-the-content-length-0.md (UpdateManagedThing 204 Content-Length) - that issue owns its own replay key and is not part of this bundle.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture iot-managed-integrations-managed-thing-error-paths step task#2 - PUT /managed-things/nonexistentmanagedthing0001 (UpdateManagedThing) for an Identifier that does not exist; observe 404 and compare $.Message against the recorded, request-derived template.
3. Cause 2: step task#3 of the same fixture - DELETE /managed-things/nonexistentmanagedthing0001 (DeleteManagedThing); observe 404 and compare $.Message the same way.
4. Cause 3: step task#4 of the same fixture - POST /managed-things (CreateManagedThing) with an AuthenticationMaterialType outside the AuthMaterialType enum; observe 400 ValidationException and compare the order of the enum value set in $.Message.
5. Cause 4: replay fixture iot-managed-integrations-provisioning-profile-lifecycle step task#3 - POST /provisioning-profiles (CreateProvisioningProfile) with a ProvisioningType outside the ProvisioningType enum; observe 400 ValidationException and compare the order of the enum value set in $.Message.
6. All four steps must pass before the issue is closed.
