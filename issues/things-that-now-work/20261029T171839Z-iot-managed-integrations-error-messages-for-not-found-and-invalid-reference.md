Issue-Source: trace-sweep-divergence
Issue-Trace: iot-managed-integrations-account-association-error-paths
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-managed-integrations-account-association-error-paths iot-managed-integrations-cloud-connector-error-paths iot-managed-integrations-event-log-configuration-wildcard-lifecycle iot-managed-integrations-ota-task-configuration-lifecycle iot-managed-integrations-notification-configuration-error-paths iot-managed-integrations-connector-destination-error-paths
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations|GetAccountAssociation|4xx->4xx|$.Message
Issue-Key: iot-managed-integrations|GetCloudConnector|4xx->4xx|$.Message
Issue-Key: iot-managed-integrations|GetEventLogConfiguration|4xx->4xx|$.Message
Issue-Key: iot-managed-integrations|GetEventLogConfiguration|4xx->4xx|$.ResourceId
Issue-Key: iot-managed-integrations|GetEventLogConfiguration|4xx->4xx|$.ResourceType
Issue-Key: iot-managed-integrations|GetOtaTaskConfiguration|4xx->4xx|$.Message
Issue-Key: iot-managed-integrations|CreateNotificationConfiguration|4xx->4xx|$.Message
Issue-Key: iot-managed-integrations|CreateConnectorDestination|4xx->4xx|$.Message
Issue-Commit: 4fccfcb9e83e9e12ca223d9fc117bac8a0390f59

# Error messages for not-found and invalid-reference paths do not match AWS wording

> _Note: this issue was identified against the code base at commit `4fccfcb9e83e9e12ca223d9fc117bac8a0390f59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccountAssociation

## Description
The emulator returns the right HTTP status and error type for these error paths but synthesises a generic message from the resource type ('<Resource> resource not found.' / 'The provided <Member> does not exist.') instead of the per-operation message AWS words. The message is part of the error contract, so each operation needs its own fixed text. This bundle has 6 causes; all six must be fixed.

Cause 1 of 6: GetAccountAssociation on an unknown AccountAssociationId.
AWS: Smithy: GetAccountAssociation declares ResourceNotFoundException (httpError 404, members Message/ResourceId/ResourceType). Recorded 404 body Message is exactly 'Account association not found.' — entirely fixed text; no request value is interpolated.
Emulator: 404 with Message 'Account Association resource not found.'
Why: clients and test suites that match on AWS's message text (or display it) see wording AWS never emits.
Fix: GetAccountAssociation's not-found error construction.

Cause 2 of 6: GetCloudConnector on an unknown identifier.
AWS: ResourceNotFoundException (404); recorded Message 'Cloud connector not found.' (fixed text).
Emulator: 404 with Message 'Cloud Connector resource not found.'
Fix: GetCloudConnector's not-found error construction.

Cause 3 of 6: GetEventLogConfiguration on an unknown Id.
AWS: ResourceNotFoundException (404); recorded Message "Event log configuration doesn't exist" (fixed text), and the recorded body carries NO ResourceId and NO ResourceType member.
Emulator: 404 with Message 'Event Log Configuration resource not found.' plus ResourceId (the requested id) and ResourceType 'EventLogConfiguration'.
Fix: GetEventLogConfiguration's not-found error construction — correct text and stop populating ResourceId/ResourceType for this operation.

Cause 4 of 6: GetOtaTaskConfiguration on an unknown Identifier.
AWS: ResourceNotFoundException (404); recorded Message 'OTA task configuration not found for id: <Identifier>' — fixed text 'OTA task configuration not found for id: ' followed by the identifier taken from the request path label.
Emulator: 404 with Message 'Ota Task Configuration resource not found.' and no identifier.
Fix: GetOtaTaskConfiguration's not-found error construction, interpolating the requested identifier.

Cause 5 of 6: CreateNotificationConfiguration naming a destination that does not exist.
AWS: Smithy: CreateNotificationConfiguration input DestinationName @required; errors include ValidationException (400, single member Message). Recorded 400 body Message "Destination doesn't exist" (fixed text, destination name not interpolated).
Emulator: 400 with Message 'The provided DestinationName does not exist.'
Fix: CreateNotificationConfiguration's validation error construction.

Cause 6 of 6: CreateConnectorDestination validation precedence and text when SecretsManager is absent.
AWS: Smithy: CreateConnectorDestination input CloudConnectorId @required, AuthConfig @required, SecretsManager optional in the model; errors include ValidationException (400). For a request missing the SecretsManager ARN, the recorded 400 Message is 'SecretsManagerArn cannot be null or empty' — i.e. the SecretsManagerArn check is reported, ahead of any cloud-connector resolution.
Emulator: 400 with Message 'The provided CloudConnectorId does not exist.' — it reports a different (later) check, so the caller is told to fix the wrong field.
Fix: CreateConnectorDestination's validation — add the SecretsManagerArn null/empty check and order it before cloud-connector resolution, with AWS's text.

Traces: the six fixture steps listed below.

## Evidence
Cause 1 — iot-managed-integrations-account-association-error-paths#task#1 (status 404 both): $.Message value_mismatch (expected 'Account association not found.', got 'Account Association resource not found.').
Cause 2 — iot-managed-integrations-cloud-connector-error-paths#task#2 (404 both): $.Message value_mismatch (expected 'Cloud connector not found.', got 'Cloud Connector resource not found.').
Cause 3 — iot-managed-integrations-event-log-configuration-wildcard-lifecycle#task#2 (404 both): $.Message value_mismatch (expected "Event log configuration doesn't exist", got 'Event Log Configuration resource not found.'); $.ResourceId type_mismatch (expected None, got the requested id); $.ResourceType type_mismatch (expected None, got 'EventLogConfiguration').
Cause 4 — iot-managed-integrations-ota-task-configuration-lifecycle#task#2 (404 both): $.Message value_mismatch (expected 'OTA task configuration not found for id: <Identifier>', got 'Ota Task Configuration resource not found.').
Cause 5 — iot-managed-integrations-notification-configuration-error-paths#task#4 (400 both): $.Message value_mismatch (expected "Destination doesn't exist", got 'The provided DestinationName does not exist.').
Cause 6 — iot-managed-integrations-connector-destination-error-paths#task#4 (400 both): $.Message value_mismatch (expected 'SecretsManagerArn cannot be null or empty', got 'The provided CloudConnectorId does not exist.').

## Steps to Reproduce
1. Replay iot-managed-integrations-account-association-error-paths#task#1: GET /account-associations/{unknown-id} and compare the 404 Message.
2. Replay iot-managed-integrations-cloud-connector-error-paths#task#2: GET /cloud-connectors/{unknown-id}.
3. Replay iot-managed-integrations-event-log-configuration-wildcard-lifecycle#task#2: GET /event-log-configurations/{unknown-id}.
4. Replay iot-managed-integrations-ota-task-configuration-lifecycle#task#2: GET /ota-task-configurations/{unknown-id}.
5. Replay iot-managed-integrations-notification-configuration-error-paths#task#4: POST /notification-configurations naming a destination that does not exist.
6. Replay iot-managed-integrations-connector-destination-error-paths#task#4: POST /connector-destinations with no SecretsManager ARN.
