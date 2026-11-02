Issue-Source: trace-sweep-divergence
Issue-Trace: iot-managed-integrations-account-association-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-managed-integrations-account-association-error-paths iot-managed-integrations-cloud-connector-error-paths iot-managed-integrations-destination-lifecycle
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations|CreateAccountAssociation|4xx->2xx|$.Message
Issue-Key: iot-managed-integrations|CreateAccountAssociation|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot-managed-integrations|CreateAccountAssociation|4xx->2xx|status
Issue-Key: iot-managed-integrations|CreateCloudConnector|4xx->2xx|$.Message
Issue-Key: iot-managed-integrations|CreateCloudConnector|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot-managed-integrations|CreateCloudConnector|4xx->2xx|status
Issue-Key: iot-managed-integrations|CreateDestination|4xx->2xx|$.Message
Issue-Key: iot-managed-integrations|CreateDestination|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot-managed-integrations|CreateDestination|4xx->2xx|status
Issue-Commit: 4fccfcb9e83e9e12ca223d9fc117bac8a0390f59

# Create operations skip referential validation and return 201 instead of ValidationException

> _Note: this issue was identified against the code base at commit `4fccfcb9e83e9e12ca223d9fc117bac8a0390f59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAccountAssociation

## Description
Three create operations accept a request whose referenced dependency does not exist (or cannot be assumed) and create the resource, returning the modeled 201 success instead of the modeled 400 ValidationException. This bundle has 3 causes; all three must be fixed.

Cause 1 of 3: CreateAccountAssociation with a ConnectorDestinationId that does not exist.
AWS: Smithy: CreateAccountAssociation input ConnectorDestinationId @required (length 1..64, pattern ^[A-Za-z0-9-_]+$); errors include ValidationException (httpError 400, single member Message). The recorded response is HTTP 400 with header x-amzn-errortype: ValidationException and body Message 'Connector Destination in the request does not exist.' — that whole sentence is fixed text for this error class (it names the member class, not the submitted id value).
Emulator: HTTP 201 with a created account association; no Message, no x-amzn-errortype header.
Why: a client that probes a bad connector-destination reference expects a 400 it can surface/retry-fix; instead it gets a 201 and believes an association exists that AWS would never have created, so subsequent Get/Delete flows are built on a phantom resource.
Fix: CreateAccountAssociation's request validation — resolve ConnectorDestinationId against connector-destination state before creating and raise ValidationException when it does not resolve.

Cause 2 of 3: CreateCloudConnector with an EndpointConfig Lambda ARN that does not exist.
AWS: Smithy: CreateCloudConnector input EndpointConfig @required; errors include ValidationException (400). Recorded response: HTTP 400, x-amzn-errortype: ValidationException, Message 'Connector endpoint Lambda ARN does not exist' (fixed text; no request values interpolated).
Emulator: HTTP 201 with a created cloud connector and no error fields.
Why: the same phantom-resource problem, and callers never learn that the Lambda target is unusable.
Fix: CreateCloudConnector's request validation — reject an EndpointConfig whose Lambda ARN is not resolvable.

Cause 3 of 3: CreateDestination with a RoleArn that is invalid / cannot be assumed.
AWS: Smithy: CreateDestination input RoleArn @required (target DeliveryDestinationRoleArn), DeliveryDestinationArn @required with pattern ^arn:aws:[0-9a-zA-Z]+:[0-9a-zA-Z-]+:[0-9]+:[0-9a-zA-Z]+/[0-9a-zA-Z._-]+$; errors include ValidationException (400). Recorded response: HTTP 400, x-amzn-errortype: ValidationException, Message 'Role ARN is invalid or cannot be assumed. Please check the role configuration and permissions.' (fixed text).
Emulator: HTTP 201 with the destination created.
Why: a destination that AWS refuses is reported as created, so notification wiring that depends on an assumable role silently appears healthy.
Fix: CreateDestination's request validation — validate RoleArn shape/assumability and raise ValidationException.

Traces: the three fixture steps listed below.

## Evidence
Cause 1 — iot-managed-integrations-account-association-error-paths#task#2: expected status 400, got 201; $.Message missing (expected 'Connector Destination in the request does not exist.'); $headers.x-amzn-errortype missing (expected 'ValidationException').
Cause 2 — iot-managed-integrations-cloud-connector-error-paths#task#1: expected status 400, got 201; $.Message missing (expected 'Connector endpoint Lambda ARN does not exist'); $headers.x-amzn-errortype missing (expected 'ValidationException').
Cause 3 — iot-managed-integrations-destination-lifecycle#task#4: expected status 400, got 201; $.Message missing (expected 'Role ARN is invalid or cannot be assumed. Please check the role configuration and permissions.'); $headers.x-amzn-errortype missing (expected 'ValidationException').

## Steps to Reproduce
1. Replay fixture iot-managed-integrations-account-association-error-paths, step task#2: POST /account-associations with a ConnectorDestinationId that was never created — emulator answers 201 instead of 400 ValidationException.
2. Replay fixture iot-managed-integrations-cloud-connector-error-paths, step task#1: POST /cloud-connectors with an EndpointConfig Lambda ARN that does not exist — emulator answers 201.
3. Replay fixture iot-managed-integrations-destination-lifecycle, step task#4: POST /destinations with a RoleArn that cannot be assumed — emulator answers 201.
