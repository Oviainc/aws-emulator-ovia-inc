Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iot__iotsitewise.cfn-deploy-iotsitewise-gateway cloudformation__iot__iotsitewise.cfn-iotsitewise-gateway-platform-oneof-violation-rejected
Issue-Key: cloudformation__iot__iotsitewise|CreateStack|ValidationError
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# CloudFormation does not recognize AWS::IoTSiteWise::Gateway, so a gateway on a GreengrassV2 core device cannot be deployed and its GatewayPlatform oneOf violation is never schema-rejected

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Every CloudFormation template that declares an IoT SiteWise gateway is refused at CreateStack as a template format error, so the cross-service IoT-thing + SiteWise-gateway deployment and its schema-rejection path are both unavailable. This issue bundles 2 causes; both must be closed. Related open issue 20261102T084333Z-iotsitewise-cloudformation-refuses-aws-iotsitewise-assetmodel-as-an-unrecognized.md records the same registry gap for AWS::IoTSiteWise::AssetModel only; it does not name AWS::IoTSiteWise::Gateway and does not cover either check below (neither was marked OWNED/ALREADY FILED by the replay).

Cause 1 of 2: AWS::IoTSiteWise::Gateway is not a registered resource type, so a stack that creates an AWS::IoT::Thing and a SiteWise gateway bound to it as its GreengrassV2 core device cannot be created.
AWS: recorded behavior only — the resource-type registry is a CloudFormation fact, not part of the iotsitewise Smithy model; the operation it must drive IS modeled (Smithy: CreateGateway POST /20200301/gateways code 201, input gatewayName @required (length 1..256) and gatewayPlatform @required, output gatewayId/gatewayArn @required; DescribeGateway; ListGateways). Real AWS accepts a CreateStack whose Resources declare {Type: AWS::IoT::Thing} with ThingName and {Type: AWS::IoTSiteWise::Gateway} with GatewayName and GatewayPlatform.GreengrassV2.CoreDeviceThingName: !Ref Thing. The stack reaches CREATE_COMPLETE, Ref Thing resolves to the thing name and Ref Gateway to the created gatewayId, IoT DescribeThing reports that thingName with thingArn arn:<partition>:iot:<region>:<account>:thing/<thingName>, SiteWise DescribeGateway reports the gatewayName with gatewayPlatform carrying ONLY greengrassV2.coreDeviceThingName equal to the thing, and ListGateways lists that gateway once under the Ref'd id.
Emulator: CreateStack is refused with HTTP 400 ValidationError and the message 'Template format error: Unrecognized resource types: [AWS::IoTSiteWise::Gateway]'. Fixed text: 'Template format error: Unrecognized resource types: ['...']'; the variable part is the list of unrecognized type names taken from the request template.
Why: SiteWise edge estates are provisioned by template together with their IoT things; a client's stack is refused outright, and the message blames the client's template rather than disclosing an unimplemented resource type.
Fix: the CloudFormation crate's resource-type registry and provisioner — register AWS::IoTSiteWise::Gateway, map GatewayName/GatewayPlatform (GreengrassV2.CoreDeviceThingName, SiemensIE.IotCoreThingName) onto CreateGateway, resolve Ref to the created gatewayId, and support update/delete so stack deletion removes the gateway.
Traces: cloudformation__iot__iotsitewise.cfn-deploy-iotsitewise-gateway.

Cause 2 of 2: a GatewayPlatform naming BOTH platforms at once is refused at CreateStack as a template format error instead of being accepted and rolled back with CloudFormation's own model-validation failure on the resource.
AWS: recorded behavior only — GatewayPlatform is a documented oneOf (exactly one of GreengrassV2 / SiemensIE), enforced by the resource type's schema, not by the iotsitewise Smithy model. A template whose GatewayPlatform carries both GreengrassV2.CoreDeviceThingName and SiemensIE.IotCoreThingName is ACCEPTED by CreateStack; the Gateway resource then fails schema validation, the stack ends ROLLBACK_COMPLETE, and the single CREATE_FAILED event for logical id Gateway carries a resource-handler reason whose message is 'Model validation failed (#/GatewayPlatform: #: only 1 subschema matches out of 2)\n#/GatewayPlatform: #: 2 subschemas matched instead of one (#/GatewayPlatform)' with HandlerErrorCode 'InvalidRequest' — entirely fixed text, with the JSON pointer #/GatewayPlatform naming the offending member. The rejected gateway never reaches IoT SiteWise (absent from ListGateways while the one-platform control gateway is still listed) and the rollback removes the stack's thing from IoT, so DescribeThing answers ResourceNotFoundException 'Thing <account>:<thingName> not found.'
Emulator: the same ValidationError 'Template format error: Unrecognized resource types: [AWS::IoTSiteWise::Gateway]' at CreateStack — the positive control never deploys, so neither the ROLLBACK_COMPLETE path, the model-validation handler reason, nor the cross-service rollback of the IoT thing exists.
Why: a client expects an over-specified platform to fail as a stack rollback carrying the schema diagnosis, with every resource of the stack (including the IoT thing) cleaned up; here it gets a generic template-format rejection, so neither the diagnosis nor the rollback-cleans-up guarantee can be relied on.
Fix: the CloudFormation provisioner's schema-validation and rollback path for the newly registered AWS::IoTSiteWise::Gateway type — validate GatewayPlatform's oneOf before calling CreateGateway, record the CREATE_FAILED reason in resource-handler form with HandlerErrorCode InvalidRequest and the model-validation message above, and roll the stack back so the IoT thing is deleted and no gateway is created.
Traces: cloudformation__iot__iotsitewise.cfn-iotsitewise-gateway-platform-oneof-violation-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator from this checkout: 55/91 passed. Service cloudformation__iot__iotsitewise: 0/2 (0%), both checks errored.

1) cloudformation__iot__iotsitewise.cfn-deploy-iotsitewise-gateway — cloudformation.create_stack with a template declaring AWS::IoT::Thing (ThingName !Sub '${AWS::StackName}-thing') and AWS::IoTSiteWise::Gateway (GatewayName !Sub '${AWS::StackName}-gw', GatewayPlatform.GreengrassV2.CoreDeviceThingName !Ref Thing), Outputs ThingName/GatewayId.
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::IoTSiteWise::Gateway]
Expected: CreateStack accepted, stack CREATE_COMPLETE, Ref outputs resolving, iot.describe_thing reporting the thing and its arn:<partition>:iot:<region>:<account>:thing/<name>, sitewise.describe_gateway reporting gatewayName with gatewayPlatform keys == ['greengrassV2'] and coreDeviceThingName == the thing, and ListGateways listing it once.

2) cloudformation__iot__iotsitewise.cfn-iotsitewise-gateway-platform-oneof-violation-rejected — same template plus 'SiemensIE: {IotCoreThingName: !Ref Thing}' under GatewayPlatform.
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::IoTSiteWise::Gateway] (raised on the positive-control stack, before the two-platform stack is even attempted).
Expected: the control stack CREATE_COMPLETE as in (1); the two-platform stack accepted and ending ROLLBACK_COMPLETE with failed_reason(events,'Gateway') == ('Model validation failed (#/GatewayPlatform: #: only 1 subschema matches out of 2)\n#/GatewayPlatform: #: 2 subschemas matched instead of one (#/GatewayPlatform)', 'InvalidRequest') after masking; the rejected gateway absent from ListGateways, the control gateway still listed, and iot.describe_thing on the rolled-back thing answering ('ResourceNotFoundException', 'Thing <account>:<thingName> not found.').

Stack names, thing names, gateway ids and account/region are per-run values filling the templates; the requirements are that AWS::IoTSiteWise::Gateway is recognized and provisioned, that Ref resolves to the gatewayId, and that a oneOf violation becomes a ROLLBACK_COMPLETE carrying the model-validation reason above.

## Steps to Reproduce
1. Call CloudFormation CreateStack with a template whose Resources are an AWS::IoT::Thing and an AWS::IoTSiteWise::Gateway with GatewayPlatform.GreengrassV2.CoreDeviceThingName: !Ref Thing, plus Outputs !Ref Thing and !Ref Gateway.
2. Observe the call refused with ValidationError 'Template format error: Unrecognized resource types: [AWS::IoTSiteWise::Gateway]' instead of being accepted and reaching CREATE_COMPLETE with a describable gateway whose gatewayPlatform contains only greengrassV2.
3. Repeat with GatewayPlatform naming BOTH GreengrassV2 and SiemensIE. Observe the same template-format refusal instead of an accepted stack that ends ROLLBACK_COMPLETE with the Gateway resource's CREATE_FAILED reason 'Model validation failed (#/GatewayPlatform: #: only 1 subschema matches out of 2)\n#/GatewayPlatform: #: 2 subschemas matched instead of one (#/GatewayPlatform)' / HandlerErrorCode InvalidRequest, no gateway in ListGateways, and the stack's IoT thing deleted by the rollback.
4. Re-replay cloudformation__iot__iotsitewise.cfn-deploy-iotsitewise-gateway and cloudformation__iot__iotsitewise.cfn-iotsitewise-gateway-platform-oneof-violation-rejected.
