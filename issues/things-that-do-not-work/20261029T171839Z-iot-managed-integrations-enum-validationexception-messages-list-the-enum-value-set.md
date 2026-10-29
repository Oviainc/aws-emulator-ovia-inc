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
Issue-Commit: 4fccfcb9e83e9e12ca223d9fc117bac8a0390f59

# Enum ValidationException messages list the enum value set in the wrong order

> _Note: this issue was identified against the code base at commit `4fccfcb9e83e9e12ca223d9fc117bac8a0390f59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateManagedThing

## Description
When an enum-typed member fails its constraint the emulator produces AWS's message template correctly except for the order of the enumerated values, which it renders in Smithy declaration order while AWS renders the reverse. The value list is part of the message's fixed text for that error class, so a client or test matching the message fails. This bundle has 2 causes; both must be fixed.

Cause 1 of 2: CreateManagedThing with an invalid AuthenticationMaterialType.
AWS: Smithy: CreateManagedThing input AuthenticationMaterialType @required, target enum AuthMaterialType declared as [CUSTOM_PROTOCOL_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE, ZWAVE_QR_BAR_CODE, ZIGBEE_QR_BAR_CODE, DISCOVERED_DEVICE, PRE_ONBOARDED_CLOUD]; errors include ValidationException (httpError 400). The recorded 400 Message is "1 validation error detected: Value at 'authenticationMaterialType' failed to satisfy constraint: Member must satisfy enum value set: [PRE_ONBOARDED_CLOUD, ZIGBEE_QR_BAR_CODE, ZWAVE_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE, DISCOVERED_DEVICE, CUSTOM_PROTOCOL_QR_BAR_CODE]": the fixed parts are the count prefix, the constraint sentence and the bracketed value set; the variable parts are the error count and the member name (here the lowerCamel wire name of the failing member). AWS lists the set in the reverse of the model's declaration order (recorded behavior; the model does not state an ordering).
Emulator: identical status (400), error type and message text, but the set is rendered in declaration order: [CUSTOM_PROTOCOL_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE, ZWAVE_QR_BAR_CODE, ZIGBEE_QR_BAR_CODE, DISCOVERED_DEVICE, PRE_ONBOARDED_CLOUD].
Why: clients and conformance suites that assert on the validation message — the usual way this class of error is checked, since the body carries only Message — mismatch on every enum rejection.
Fix: the shared enum-constraint validation message formatter (the component that renders 'Member must satisfy enum value set'), so the ordering matches AWS for every enum member.

Cause 2 of 2: CreateProvisioningProfile with an invalid ProvisioningType.
AWS: Smithy: CreateProvisioningProfile input ProvisioningType @required, target enum ProvisioningType declared as [FLEET_PROVISIONING, JITR]; ValidationException (400). Recorded Message: "1 validation error detected: Value at 'provisioningType' failed to satisfy constraint: Member must satisfy enum value set: [JITR, FLEET_PROVISIONING]" — again the reverse of declaration order.
Emulator: same message with [FLEET_PROVISIONING, JITR].
Why: same as cause 1.
Fix: the same enum-constraint message formatter; this cause confirms the ordering rule on a second, two-value enum.

Traces: the two fixture steps listed below.

## Evidence
Cause 1 — iot-managed-integrations-managed-thing-error-paths#task#4 (400 both): $.Message value_mismatch — expected "1 validation error detected: Value at 'authenticationMaterialType' failed to satisfy constraint: Member must satisfy enum value set: [PRE_ONBOARDED_CLOUD, ZIGBEE_QR_BAR_CODE, ZWAVE_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE, DISCOVERED_DEVICE, CUSTOM_PROTOCOL_QR_BAR_CODE]", got the same sentence with the set as [CUSTOM_PROTOCOL_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE, ZWAVE_QR_BAR_CODE, ZIGBEE_QR_BAR_CODE, DISCOVERED_DEVICE, PRE_ONBOARDED_CLOUD].
Cause 2 — iot-managed-integrations-provisioning-profile-lifecycle#task#3 (400 both): $.Message value_mismatch — expected "...Member must satisfy enum value set: [JITR, FLEET_PROVISIONING]", got "...[FLEET_PROVISIONING, JITR]".

## Steps to Reproduce
1. Replay iot-managed-integrations-managed-thing-error-paths#task#4: POST /managed-things with an AuthenticationMaterialType outside AuthMaterialType and compare the 400 Message's bracketed value set.
2. Replay iot-managed-integrations-provisioning-profile-lifecycle#task#3: POST /provisioning-profiles with a ProvisioningType outside the enum and compare the 400 Message's bracketed value set.
