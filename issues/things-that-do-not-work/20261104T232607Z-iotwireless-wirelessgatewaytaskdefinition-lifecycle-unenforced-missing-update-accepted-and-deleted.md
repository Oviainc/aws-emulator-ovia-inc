Issue-Source: trace-sweep-divergence
Issue-Trace: iot-wireless-wireless-gateway-task-definition-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-wireless-wireless-gateway-task-definition-lifecycle
Issue-Shape-Id: com.amazonaws.iotwireless#iotwireless
Issue-Key: iotwireless|CreateWirelessGatewayTaskDefinition|4xx->2xx|$.Message
Issue-Key: iotwireless|CreateWirelessGatewayTaskDefinition|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iotwireless|CreateWirelessGatewayTaskDefinition|4xx->2xx|status
Issue-Key: iotwireless|GetWirelessGatewayTaskDefinition|4xx->2xx|$.Message
Issue-Key: iotwireless|GetWirelessGatewayTaskDefinition|4xx->2xx|$.ResourceId
Issue-Key: iotwireless|GetWirelessGatewayTaskDefinition|4xx->2xx|$.ResourceType
Issue-Key: iotwireless|GetWirelessGatewayTaskDefinition|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iotwireless|GetWirelessGatewayTaskDefinition|4xx->2xx|status
Issue-Commit: 3157dd4d1552792a0982de1a1298281fbb744f2b

# WirelessGatewayTaskDefinition lifecycle unenforced: missing Update accepted and deleted definition stays readable

> _Note: this issue was identified against the code base at commit `3157dd4d1552792a0982de1a1298281fbb744f2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWirelessGatewayTaskDefinition

## Description
The emulator's WirelessGatewayTaskDefinition resource accepts a creation request AWS refuses and keeps serving a definition AWS has deleted. Two causes, both on the iotwireless WirelessGatewayTaskDefinition resource.

Cause 1 of 2: CreateWirelessGatewayTaskDefinition accepts a request that omits `Update`.
AWS: Smithy iotwireless CreateWirelessGatewayTaskDefinition — POST /wireless-gateway-task-definitions, success code 201, input members AutoCreateTasks (required, boolean, default false), Name, Update (UpdateWirelessGatewayTaskCreate), ClientRequestToken, Tags; modeled errors include ValidationException (httpError 400). `Update` is not marked @required in the model, but the recorded response shows AWS refuses a definition with no update payload: HTTP 400 with body member `Message` = "Parameter Update is required." and an `x-amzn-errortype` response header naming the modeled error shape. The message is entirely fixed text apart from the parameter name `Update`, which is the input member that was omitted — i.e. the template is "Parameter <memberName> is required." and `Update` is the value taken from the request shape.
Emulator: returns HTTP 201 and creates the definition, emitting Id/Arn. No `Message`, no `x-amzn-errortype` header, no refusal at all.
Why: a client that relies on AWS rejecting an update-less task definition sees a success and an Id for a definition that could never exist on AWS. Code paths that handle the 400 (prompting for the firmware update block, surfacing the validation error) are never exercised, and subsequent gateway-task creation against that bogus definition behaves differently from AWS.
Fix: the CreateWirelessGatewayTaskDefinition handler's request validation — add the presence check on `Update` before the definition is inserted into state, and have it raise the service's 400 validation refusal carrying the `Message` body member and the `x-amzn-errortype` header.
Traces: iot-wireless-wireless-gateway-task-definition-lifecycle#task#5 (expected 400, got 201).

Cause 2 of 2: GetWirelessGatewayTaskDefinition still returns a deleted definition.
AWS: Smithy iotwireless DeleteWirelessGatewayTaskDefinition — DELETE /wireless-gateway-task-definitions/{Id}, success code 204; GetWirelessGatewayTaskDefinition — GET /wireless-gateway-task-definitions/{Id}, success code 200, modeled error ResourceNotFoundException (httpError 404). After the definition is deleted, a Get on the same Id is a 404 ResourceNotFoundException whose body carries `Message`, `ResourceId` and `ResourceType`, plus the `x-amzn-errortype` header. The message AWS words for this error class is "Cannot complete the step because resource is not found, with resource type <ResourceType>, resource Id <ResourceId>" — the fixed text is everything but the two substituted values; `ResourceType` is the constant "WirelessGatewayTaskDefinition" for this resource, and `ResourceId` is the Id taken verbatim from the request path label (the recording's UUID is one run's value, not part of the requirement). `ResourceId` and `ResourceType` appear as body members with those same two values.
Emulator: returns HTTP 200 with the definition body as if it were still present — the delete did not remove it from state (or Get does not consult the deletion). None of `Message`, `ResourceId`, `ResourceType` or `x-amzn-errortype` is present.
Why: deletion is not observable, so a client cannot tell a live definition from a deleted one. Cleanup verification loops that poll for the 404 never terminate, idempotent-delete and recreate flows see stale state, and error handling for ResourceNotFoundException is never reached.
Fix: the WirelessGatewayTaskDefinition state store plus the GetWirelessGatewayTaskDefinition handler — make DeleteWirelessGatewayTaskDefinition actually remove the record and have Get raise the service's 404 ResourceNotFoundException (with Message/ResourceId/ResourceType and the error-type header) for an absent Id.
Traces: iot-wireless-wireless-gateway-task-definition-lifecycle#task#7 (expected 404, got 200).

No open issue covers this resource: the existing iotwireless issues cover DeviceProfile (20261030T072116Z), MulticastGroup (20261030T115132Z), WirelessGateway (20261103T042841Z), ServiceProfile (20261103T125153Z), WirelessDevice (20261103T125153Z / 20261103T121946Z) and the generic validation-message wording (20261029T171855Z) — none of them carries a WirelessGatewayTaskDefinition replay key.

## Evidence
Cause 1 — fixture iot-wireless-wireless-gateway-task-definition-lifecycle, step task#5, CreateWirelessGatewayTaskDefinition with no Update member:
  expected: status 400; $.Message = "Parameter Update is required."; $headers.x-amzn-errortype present
  actual:   status 201; $.Message missing; $headers.x-amzn-errortype missing
  divergence_key: service=iotwireless|CreateWirelessGatewayTaskDefinition|status=400->201|keys=$.Message,$headers.x-amzn-errortype,status

Cause 2 — fixture iot-wireless-wireless-gateway-task-definition-lifecycle, step task#7, GetWirelessGatewayTaskDefinition after DeleteWirelessGatewayTaskDefinition:
  expected: status 404; $.Message = "Cannot complete the step because resource is not found, with resource type WirelessGatewayTaskDefinition, resource Id <definitionId>"; $.ResourceId = <definitionId>; $.ResourceType = "WirelessGatewayTaskDefinition"; $headers.x-amzn-errortype present
  actual:   status 200; $.Message, $.ResourceId, $.ResourceType and $headers.x-amzn-errortype all missing
  divergence_key: service=iotwireless|GetWirelessGatewayTaskDefinition|status=404->200|keys=$.Message,$.ResourceId,$.ResourceType,$headers.x-amzn-errortype,status

Model evidence (services/service-com-amazonaws-iotwireless-iotwireless/smithy-model/com-amazonaws-iotwireless-iotwireless.json): CreateWirelessGatewayTaskDefinition errors include ValidationException@400; GetWirelessGatewayTaskDefinition errors include ResourceNotFoundException@404; DeleteWirelessGatewayTaskDefinition success code 204.

## Steps to Reproduce
Replay fixture iot-wireless-wireless-gateway-task-definition-lifecycle against the emulator (replay_wire_corpus).

Cause 1 (step task#5): POST /wireless-gateway-task-definitions with a body carrying AutoCreateTasks and Name but NO Update member. Emulator answers 201 with Id/Arn; AWS answers 400 with Message "Parameter Update is required." and an x-amzn-errortype header.

Cause 2 (step task#7): create a task definition (POST /wireless-gateway-task-definitions with AutoCreateTasks, Name and a valid Update block), note the returned Id, DELETE /wireless-gateway-task-definitions/{Id} (expect 204), then GET /wireless-gateway-task-definitions/{Id}. Emulator answers 200 with the definition body; AWS answers 404 with Message "Cannot complete the step because resource is not found, with resource type WirelessGatewayTaskDefinition, resource Id {Id}", ResourceId {Id}, ResourceType "WirelessGatewayTaskDefinition" and an x-amzn-errortype header.
