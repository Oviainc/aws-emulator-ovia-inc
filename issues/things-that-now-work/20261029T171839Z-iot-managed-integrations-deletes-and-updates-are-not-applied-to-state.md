Issue-Source: trace-sweep-divergence
Issue-Trace: iot-managed-integrations-event-log-configuration-scoped-lifecycle
Issue-Step: teardown#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-managed-integrations-event-log-configuration-scoped-lifecycle iot-managed-integrations-event-log-configuration-wildcard-lifecycle iot-managed-integrations-ota-task-configuration-lifecycle
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations|GetEventLogConfiguration|4xx->2xx|$.Message
Issue-Key: iot-managed-integrations|GetEventLogConfiguration|4xx->2xx|$.ResourceId
Issue-Key: iot-managed-integrations|GetEventLogConfiguration|4xx->2xx|$.ResourceType
Issue-Key: iot-managed-integrations|GetEventLogConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot-managed-integrations|GetEventLogConfiguration|4xx->2xx|status
Issue-Key: iot-managed-integrations|GetOtaTaskConfiguration|4xx->2xx|$.Message
Issue-Key: iot-managed-integrations|GetOtaTaskConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot-managed-integrations|GetOtaTaskConfiguration|4xx->2xx|status
Issue-Key: iot-managed-integrations|GetEventLogConfiguration|2xx->2xx|$.EventLogLevel
Issue-Key: iot-managed-integrations|GetEventLogConfiguration|2xx->2xx|$.ResourceId
Issue-Commit: 4fccfcb9e83e9e12ca223d9fc117bac8a0390f59

# Deletes and updates are not applied to state for event-log and OTA task configurations

> _Note: this issue was identified against the code base at commit `4fccfcb9e83e9e12ca223d9fc117bac8a0390f59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEventLogConfiguration

## Description
Write operations on event-log configurations and OTA task configurations do not change the stored resource: after a successful Delete the resource is still readable with 200, and after a successful Update the old attribute value is still returned. This bundle has 4 causes; all four must be fixed.

Cause 1 of 4: DeleteEventLogConfiguration (resource-scoped configuration) does not remove it.
AWS: Smithy: GetEventLogConfiguration (GET /event-log-configurations/{Id}) declares ResourceNotFoundException (httpError 404). Recorded response for a Get after the configuration was deleted: HTTP 404, x-amzn-errortype: ResourceNotFoundException, body Message "Event log configuration doesn't exist" — that sentence is fixed text for this error class, with no request values interpolated, and the recorded body carries no ResourceId/ResourceType members.
Emulator: HTTP 200 with the configuration body still present (including ResourceId of the scoped credential-locker and ResourceType 'credential-locker'); no Message and no x-amzn-errortype header.
Why: a client cannot tell that its delete took effect; a teardown-then-recreate flow sees a stale configuration and a conflict/idempotency decision made from the Get is wrong.
Fix: the DeleteEventLogConfiguration handler's state mutation — remove the record so later reads resolve to ResourceNotFoundException.

Cause 2 of 4: the same Delete-not-applied defect for a wildcard-scoped configuration (ResourceType '*').
AWS: same operation and error class as cause 1: HTTP 404, x-amzn-errortype: ResourceNotFoundException, Message "Event log configuration doesn't exist".
Emulator: HTTP 200 with the wildcard configuration body (ResourceType '*') still readable.
Why: same as cause 1; the wildcard configuration is account-global, so a stale one keeps reporting log settings the account no longer has.
Fix: same handler/state path, exercised for the wildcard scope.

Cause 3 of 4: DeleteOtaTaskConfiguration does not remove the configuration.
AWS: Smithy: GetOtaTaskConfiguration (GET /ota-task-configurations/{Identifier}) declares ResourceNotFoundException (404). Recorded response after the delete: HTTP 404, x-amzn-errortype: ResourceNotFoundException, Message 'OTA task configuration not found for id: <Identifier>' — fixed text 'OTA task configuration not found for id: ' plus the identifier taken from the request path.
Emulator: HTTP 200 still serving the task configuration.
Why: deleted OTA configurations keep resolving, so clients re-use a configuration AWS considers gone.
Fix: the DeleteOtaTaskConfiguration handler's state mutation.

Cause 4 of 4: UpdateEventLogConfiguration does not persist EventLogLevel (wildcard configuration).
AWS: Smithy: UpdateEventLogConfiguration (PATCH /event-log-configurations/{Id}) takes EventLogLevel @required (enum LogLevel) and is @idempotent; a following GetEventLogConfiguration returns the updated level. Recording shows EventLogLevel 'ERROR' after the update, and the same Get also returns ResourceId 'unknown' for a wildcard scope.
Emulator: the Get returns the pre-update EventLogLevel 'INFO' and omits ResourceId entirely.
Why: a client that lowers/raises log verbosity reads back the old level and believes the change was rejected, or loops re-applying it.
Fix: the UpdateEventLogConfiguration handler's state mutation (apply EventLogLevel to the stored record); the omitted ResourceId on wildcard reads is the serializer defect filed separately for GetEventLogConfiguration wildcard responses.

Traces: the four fixture steps listed below.

## Evidence
Cause 1 — iot-managed-integrations-event-log-configuration-scoped-lifecycle#teardown#6: status expected 404, got 200; $.Message missing (expected "Event log configuration doesn't exist"); $headers.x-amzn-errortype missing (expected 'ResourceNotFoundException'); emulator additionally returned $.ResourceId and $.ResourceType ('credential-locker') where the recording has none.
Cause 2 — iot-managed-integrations-event-log-configuration-wildcard-lifecycle#teardown#8: status expected 404, got 200; $.Message missing (expected "Event log configuration doesn't exist"); $headers.x-amzn-errortype missing (expected 'ResourceNotFoundException'); $.ResourceType returned '*' where the recording has none.
Cause 3 — iot-managed-integrations-ota-task-configuration-lifecycle#teardown#7: status expected 404, got 200; $.Message missing (expected 'OTA task configuration not found for id: <Identifier>'); $headers.x-amzn-errortype missing (expected 'ResourceNotFoundException').
Cause 4 — iot-managed-integrations-event-log-configuration-wildcard-lifecycle#task#6: $.EventLogLevel value_mismatch (expected 'ERROR', got 'INFO'); $.ResourceId missing (expected 'unknown').

## Steps to Reproduce
1. Replay fixture iot-managed-integrations-event-log-configuration-scoped-lifecycle through teardown#6: create a resource-scoped event log configuration, DELETE it, then GET it — emulator answers 200 with the configuration instead of 404 ResourceNotFoundException.
2. Replay fixture iot-managed-integrations-event-log-configuration-wildcard-lifecycle through teardown#8: same sequence for a wildcard ('*') configuration.
3. Replay fixture iot-managed-integrations-ota-task-configuration-lifecycle through teardown#7: create an OTA task configuration, DELETE it, then GET it — emulator answers 200.
4. Replay fixture iot-managed-integrations-event-log-configuration-wildcard-lifecycle step task#6: PATCH the wildcard configuration to EventLogLevel ERROR, then GET it — emulator still reports INFO and omits ResourceId.
