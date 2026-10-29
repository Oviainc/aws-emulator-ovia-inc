Issue-Source: trace-sweep-divergence
Issue-Trace: iot-managed-integrations-destination-lifecycle
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-managed-integrations-destination-lifecycle iot-managed-integrations-notification-configuration-error-paths
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations|GetDestination|4xx->4xx|$.Message
Issue-Key: iot-managed-integrations|GetDestination|4xx->4xx|$.ResourceId
Issue-Key: iot-managed-integrations|GetDestination|4xx->4xx|$.ResourceType
Issue-Key: iot-managed-integrations|UpdateDestination|4xx->4xx|$.Message
Issue-Key: iot-managed-integrations|UpdateDestination|4xx->4xx|$.ResourceId
Issue-Key: iot-managed-integrations|UpdateDestination|4xx->4xx|$.ResourceType
Issue-Key: iot-managed-integrations|DeleteDestination|4xx->4xx|$.Message
Issue-Key: iot-managed-integrations|DeleteDestination|4xx->4xx|$.ResourceId
Issue-Key: iot-managed-integrations|DeleteDestination|4xx->4xx|$.ResourceType
Issue-Key: iot-managed-integrations|GetNotificationConfiguration|4xx->4xx|$.Message
Issue-Key: iot-managed-integrations|GetNotificationConfiguration|4xx->4xx|$.ResourceId
Issue-Key: iot-managed-integrations|GetNotificationConfiguration|4xx->4xx|$.ResourceType
Issue-Key: iot-managed-integrations|UpdateNotificationConfiguration|4xx->4xx|$.Message
Issue-Key: iot-managed-integrations|UpdateNotificationConfiguration|4xx->4xx|$.ResourceId
Issue-Key: iot-managed-integrations|UpdateNotificationConfiguration|4xx->4xx|$.ResourceType
Issue-Commit: 4fccfcb9e83e9e12ca223d9fc117bac8a0390f59

# ResourceNotFoundException bodies carry ResourceId, ResourceType and Message that AWS omits

> _Note: this issue was identified against the code base at commit `4fccfcb9e83e9e12ca223d9fc117bac8a0390f59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDestination

## Description
For destination and notification-configuration operations the emulator gets the 404 status and the x-amzn-errortype header right, but serialises a fully populated ResourceNotFoundException body (Message, ResourceId, ResourceType) where AWS serialises none of those members. ResourceNotFoundException's members are all optional in the model, and for these operations the recordings show them absent, so a client reading them gets fields AWS never sends. This bundle has 5 causes; all five must be fixed.

Cause 1 of 5: GetDestination on a destination name that does not exist.
AWS: Smithy: GetDestination (GET /destinations/{Name}) declares ResourceNotFoundException (httpError 404) whose members Message, ResourceId and ResourceType are all optional. Recorded response: HTTP 404 with x-amzn-errortype: ResourceNotFoundException and none of Message/ResourceId/ResourceType in the body.
Emulator: HTTP 404 with Message 'Destination resource not found.', ResourceId set to the requested destination name and ResourceType 'Destination'.
Why: a client that branches on the presence of ResourceId/ResourceType, or surfaces Message, behaves differently against the emulator than against AWS — and the values it reads (a generic message, an echoed name) are emulator inventions.
Fix: GetDestination's error serializer — emit the bare ResourceNotFoundException for this operation.

Cause 2 of 5: UpdateDestination on a destination that does not exist.
AWS: same error class and the same empty body (404, x-amzn-errortype: ResourceNotFoundException, no members).
Emulator: 404 with Message 'Destination cannot be found.', ResourceId = requested name, ResourceType 'Destination'.
Fix: UpdateDestination's error serializer.

Cause 3 of 5: DeleteDestination on a destination that does not exist.
AWS: same error class and the same empty body.
Emulator: 404 with Message 'Destination cannot be found.', ResourceId = requested name, ResourceType 'Destination'.
Fix: DeleteDestination's error serializer.

Cause 4 of 5: GetNotificationConfiguration for an event type with no configuration.
AWS: ResourceNotFoundException (404) with x-amzn-errortype set and no Message/ResourceId/ResourceType.
Emulator: 404 with Message 'Notification Configuration resource not found.', ResourceId = the requested EventType value, ResourceType 'NotificationConfiguration'.
Fix: GetNotificationConfiguration's error serializer.

Cause 5 of 5: UpdateNotificationConfiguration for an event type with no configuration.
AWS: same as cause 4 — bare 404 ResourceNotFoundException.
Emulator: 404 with Message 'Notification Configuration resource not found.', ResourceId = the requested EventType value, ResourceType 'NotificationConfiguration'.
Fix: UpdateNotificationConfiguration's error serializer.

Traces: the five fixture steps listed below.

## Evidence
Cause 1 — iot-managed-integrations-destination-lifecycle#task#1 (404 both): $.Message type_mismatch (expected None, got 'Destination resource not found.'); $.ResourceId type_mismatch (expected None, got the requested destination name); $.ResourceType type_mismatch (expected None, got 'Destination').
Cause 2 — iot-managed-integrations-destination-lifecycle#task#2 (404 both): $.Message type_mismatch (expected None, got 'Destination cannot be found.'); $.ResourceId and $.ResourceType likewise present where expected None.
Cause 3 — iot-managed-integrations-destination-lifecycle#task#3 (404 both): same three extra members, Message 'Destination cannot be found.'
Cause 4 — iot-managed-integrations-notification-configuration-error-paths#task#1 (404 both): $.Message type_mismatch (expected None, got 'Notification Configuration resource not found.'); $.ResourceId (expected None, got the requested event type); $.ResourceType (expected None, got 'NotificationConfiguration').
Cause 5 — iot-managed-integrations-notification-configuration-error-paths#task#2 (404 both): identical three extra members on the update path.

## Steps to Reproduce
1. Replay iot-managed-integrations-destination-lifecycle steps task#1, task#2 and task#3: GET, PUT/PATCH and DELETE /destinations/{name} for a destination that was never created — each 404 body carries Message/ResourceId/ResourceType that the recording does not.
2. Replay iot-managed-integrations-notification-configuration-error-paths steps task#1 and task#2: GET and PATCH /notification-configurations/{eventType} with no configuration present — same three extra members.
