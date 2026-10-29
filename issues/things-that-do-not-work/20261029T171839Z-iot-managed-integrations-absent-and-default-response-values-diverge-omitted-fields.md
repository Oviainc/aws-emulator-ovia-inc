Issue-Source: trace-sweep-divergence
Issue-Trace: iot-managed-integrations-event-log-configuration-wildcard-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-managed-integrations-event-log-configuration-wildcard-lifecycle iot-managed-integrations-credential-locker-lifecycle iot-managed-integrations-connector-destination-error-paths
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations|GetCredentialLocker|2xx->2xx|$.Id
Issue-Key: iot-managed-integrations|GetCredentialLocker|2xx->2xx|$.Tags
Issue-Key: iot-managed-integrations|DeleteConnectorDestination|2xx->4xx|$
Issue-Key: iot-managed-integrations|DeleteConnectorDestination|2xx->4xx|status
Issue-Related: 20261029T171839Z-iot-managed-integrations-deletes-and-updates-are-not-applied-to-state.md
Issue-Commit: 4fccfcb9e83e9e12ca223d9fc117bac8a0390f59

# Absent and default response values diverge: omitted fields and non-idempotent delete

> _Note: this issue was identified against the code base at commit `4fccfcb9e83e9e12ca223d9fc117bac8a0390f59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEventLogConfiguration

## Description
Three divergences in how the emulator represents values that are empty, defaulted or already gone: it drops members AWS always serialises, and it fails a delete of an absent resource that AWS treats as a success. This bundle has 3 causes; all three must be fixed.

Cause 1 of 3: GetEventLogConfiguration on a wildcard-scoped configuration omits ResourceId.
AWS: Smithy: GetEventLogConfigurationResponse has ResourceId (target SmartHomeResourceId, length 1..200, pattern ^[a-zA-Z0-9+*]*$) and ResourceType (pattern ^[*]$|^(managed-thing|credential-locker|provisioning-profile|ota-task|account-association)$). The recording shows that when the configuration's scope is the wildcard '*', AWS still serialises ResourceId, with the literal value 'unknown' (recorded behavior only — the model does not document the wildcard placeholder).
Emulator: the response omits ResourceId entirely for wildcard-scoped configurations.
Why: an SDK client reading ResourceId gets None where AWS guarantees a string, so code that formats or compares the scope hits a null where AWS never produces one.
Fix: the GetEventLogConfiguration response serializer — emit ResourceId 'unknown' for a wildcard-scoped configuration.

Cause 2 of 3: GetCredentialLocker omits an empty Tags map.
AWS: Smithy: GetCredentialLockerResponse member Tags targets TagsMap (map, length 1..50, sensitive). The recording shows Tags present as an empty object {} for a locker created with no tags (recorded behavior only).
Emulator: Tags is absent from the response body.
Why: a client that iterates response Tags without a null check works against AWS and breaks against the emulator; a client comparing tag state cannot distinguish 'no tags' from 'not reported'.
Fix: the GetCredentialLocker response serializer — always serialise Tags, empty when the locker has none.

Cause 3 of 3: DeleteConnectorDestination on an identifier that does not exist returns 404 instead of success.
AWS: Smithy: DeleteConnectorDestination (DELETE /connector-destinations/{Identifier}, success code 200, output Unit) is marked @idempotent and does declare ResourceNotFoundException; the recording for an identifier that was never created is HTTP 200 with an empty body {}.
Emulator: HTTP 404 with a ResourceNotFoundException body (__type ResourceNotFoundException, Message 'Connector destination cannot be found.', ResourceId = the requested identifier, ResourceType 'ConnectorDestination').
Why: idempotent cleanup code that deletes unconditionally succeeds against AWS and fails against the emulator, so teardown paths abort on an already-absent resource.
Fix: the DeleteConnectorDestination handler — treat an absent identifier as a successful no-op returning 200 with an empty body.

Traces: the three fixture steps listed below.

## Evidence
Cause 1 — iot-managed-integrations-event-log-configuration-wildcard-lifecycle#task#3 (200 both): $.ResourceId missing (expected 'unknown', got None).
Cause 2 — iot-managed-integrations-credential-locker-lifecycle#task#3 (200 both): $.Tags missing (expected {}, got None).
Cause 3 — iot-managed-integrations-connector-destination-error-paths#task#3: status expected 200, got 404; body expected {} but got {'__type': 'ResourceNotFoundException', 'Message': 'Connector destination cannot be found.', 'ResourceId': <requested identifier>, 'ResourceType': 'ConnectorDestination'}.

## Steps to Reproduce
1. Replay iot-managed-integrations-event-log-configuration-wildcard-lifecycle#task#3: create a wildcard ('*') event log configuration and GET it — ResourceId is missing instead of 'unknown'.
2. Replay iot-managed-integrations-credential-locker-lifecycle#task#3: create a credential locker with no tags and GET it — Tags is absent instead of {}.
3. Replay iot-managed-integrations-connector-destination-error-paths#task#3: DELETE /connector-destinations/{identifier} for an identifier that does not exist — emulator answers 404 instead of 200 with an empty body.
