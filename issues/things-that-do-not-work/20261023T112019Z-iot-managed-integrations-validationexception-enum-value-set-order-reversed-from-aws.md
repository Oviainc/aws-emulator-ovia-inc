Issue-Source: trace-sweep-divergence
Issue-Trace: iot-managed-integrations-managed-thing-error-paths
Issue-Step: task#4
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-managed-integrations-managed-thing-error-paths iot-managed-integrations-provisioning-profile-lifecycle
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations|CreateManagedThing|4xx->4xx|$.Message
Issue-Key: iot-managed-integrations|CreateProvisioningProfile|4xx->4xx|$.Message
Issue-Commit: 141a6c66c06fa968009d8f2ba54c49bf769a77dc

# ValidationException enum value-set order reversed from AWS in validation messages

> _Note: this issue was identified against the code base at commit `141a6c66c06fa968009d8f2ba54c49bf769a77dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateManagedThing

## Description
This issue bundles 2 causes, both of which are the SAME root fault: when an input enum member fails validation, the emulator formats the "Member must satisfy enum value set: [...]" list in the Smithy model's declaration order, while AWS emits that list in the reverse of the model's declaration order. The HTTP status (400) and the error shape (ValidationException) are correct in both causes; only the ordering inside the generated message body diverges. A fixer must close BOTH causes below before submitting.

Cause 1 of 2: CreateManagedThing, rejected AuthenticationMaterialType.
  AWS: CreateManagedThing (POST /managed-things, success code 201) models AuthenticationMaterialType as a @required member targeting the enum AuthMaterialType and declares ValidationException with httpError 400 (Smithy: CreateManagedThing.AuthenticationMaterialType -> AuthMaterialType, required; error ValidationException @httpError 400). The message is not documented in the model, so its text is recorded behavior only; it is however built from the request (it names the failing member path and its constraint), so the generating rule is a requirement. The recording returns 400 with the message "1 validation error detected: Value at 'authenticationMaterialType' failed to satisfy constraint: Member must satisfy enum value set: [PRE_ONBOARDED_CLOUD, ZIGBEE_QR_BAR_CODE, ZWAVE_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE, DISCOVERED_DEVICE, CUSTOM_PROTOCOL_QR_BAR_CODE]" — i.e. the value set enumerated in the REVERSE of the model's declaration order, which for AuthMaterialType is CUSTOM_PROTOCOL_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE, ZWAVE_QR_BAR_CODE, ZIGBEE_QR_BAR_CODE, DISCOVERED_DEVICE, PRE_ONBOARDED_CLOUD.
  Emulator: same status (400) and same message template, but the value set is emitted in model declaration order: "...Member must satisfy enum value set: [CUSTOM_PROTOCOL_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE, ZWAVE_QR_BAR_CODE, ZIGBEE_QR_BAR_CODE, DISCOVERED_DEVICE, PRE_ONBOARDED_CLOUD]". Replay reports $.Message value_mismatch.
  Why: a client (or a test/fixture asserting on the validation message, or a UI surfacing the allowed-value list to a user) sees the permitted values in a different order than AWS produces, so byte-for-byte message comparisons against recorded AWS responses fail and any ordering-sensitive presentation of the allowed-value list differs from production.
  Fix: the enum-constraint validation message builder used by CreateManagedThing input validation — the shared formatter that renders "Member must satisfy enum value set: [...]" must iterate the enum's values in reverse declaration order, not declaration order.
  Traces: iot-managed-integrations-managed-thing-error-paths#task#4.

Cause 2 of 2: CreateProvisioningProfile, rejected ProvisioningType.
  AWS: CreateProvisioningProfile (POST /provisioning-profiles, success code 201) models ProvisioningType as a @required member targeting the enum ProvisioningType and declares ValidationException with httpError 400 (Smithy: CreateProvisioningProfile.ProvisioningType -> ProvisioningType, required; error ValidationException @httpError 400). Message text is recorded behavior only, but is built from the request and so its generating rule is a requirement. The recording returns 400 with "1 validation error detected: Value at 'provisioningType' failed to satisfy constraint: Member must satisfy enum value set: [JITR, FLEET_PROVISIONING]" — again the reverse of the model declaration order, which is FLEET_PROVISIONING, JITR. This second, independently recorded enum confirms the ordering rule rather than being an artifact of one recording.
  Emulator: same status (400) and template, value set in declaration order: "...Member must satisfy enum value set: [FLEET_PROVISIONING, JITR]". Replay reports $.Message value_mismatch.
  Why: same client impact as cause 1 — the allowed-value list a caller is handed for an invalid provisioning type does not match what AWS returns.
  Fix: same component as cause 1 (the enum-constraint validation message formatter reached from CreateProvisioningProfile input validation); fixing the shared formatter should close both causes, but both fixtures must be re-replayed to confirm.
  Traces: iot-managed-integrations-provisioning-profile-lifecycle#task#3.

Note: the only other open issue for this service (20261022T145600Z-iot-managed-integrations-updatemanagedthing-204-responses-omit-the-content-length-0.md, UpdateManagedThing 204 Content-Length) is a different fault and does not cover either cause here.

## Evidence
Cause 1 — fixture iot-managed-integrations-managed-thing-error-paths, step task#4, CreateManagedThing:
  expected status 400, actual status 400 (match)
  $.Message expected: "1 validation error detected: Value at 'authenticationMaterialType' failed to satisfy constraint: Member must satisfy enum value set: [PRE_ONBOARDED_CLOUD, ZIGBEE_QR_BAR_CODE, ZWAVE_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE, DISCOVERED_DEVICE, CUSTOM_PROTOCOL_QR_BAR_CODE]"
  $.Message actual:   "1 validation error detected: Value at 'authenticationMaterialType' failed to satisfy constraint: Member must satisfy enum value set: [CUSTOM_PROTOCOL_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE, ZWAVE_QR_BAR_CODE, ZIGBEE_QR_BAR_CODE, DISCOVERED_DEVICE, PRE_ONBOARDED_CLOUD]"
  Model order (AuthMaterialType.values): CUSTOM_PROTOCOL_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE, ZWAVE_QR_BAR_CODE, ZIGBEE_QR_BAR_CODE, DISCOVERED_DEVICE, PRE_ONBOARDED_CLOUD -> AWS emits the exact reverse; emulator emits declaration order.

Cause 2 — fixture iot-managed-integrations-provisioning-profile-lifecycle, step task#3, CreateProvisioningProfile:
  expected status 400, actual status 400 (match)
  $.Message expected: "1 validation error detected: Value at 'provisioningType' failed to satisfy constraint: Member must satisfy enum value set: [JITR, FLEET_PROVISIONING]"
  $.Message actual:   "1 validation error detected: Value at 'provisioningType' failed to satisfy constraint: Member must satisfy enum value set: [FLEET_PROVISIONING, JITR]"
  Model order (ProvisioningType.values): FLEET_PROVISIONING, JITR -> AWS emits the exact reverse; emulator emits declaration order.

Requirement being asserted is the ORDERING RULE (value set enumerated in reverse of model declaration order), not the recorded message text; the quoted strings above are examples of it. Status 400 and the ValidationException shape already match.

## Steps to Reproduce
1. Build and boot the emulator from this checkout, then replay the wire corpus (replay_wire_corpus, trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture iot-managed-integrations-managed-thing-error-paths, step task#4 — POST /managed-things with an AuthenticationMaterialType value that is not a member of AuthMaterialType. Observe 400/ValidationException, then compare $.Message's bracketed value set against the recorded one: the emulator lists the six AuthMaterialType values in model declaration order, AWS lists them reversed.
3. Cause 2: replay fixture iot-managed-integrations-provisioning-profile-lifecycle, step task#3 — POST /provisioning-profiles with a ProvisioningType value that is not FLEET_PROVISIONING or JITR. Observe 400/ValidationException with "[FLEET_PROVISIONING, JITR]" where AWS recorded "[JITR, FLEET_PROVISIONING]".
4. Both steps pass only when the enum-constraint message formatter emits the value set in reverse declaration order for every enum, so re-replay BOTH fixtures after the fix.
