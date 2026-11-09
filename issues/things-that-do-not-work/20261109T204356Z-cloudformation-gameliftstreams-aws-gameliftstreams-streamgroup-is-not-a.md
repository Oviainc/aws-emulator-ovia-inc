Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__gameliftstreams.deploy-gameliftstreams-stream-group-round-trip-and-delete cloudformation__gameliftstreams.deploy-gameliftstreams-stream-group-invalid-stream-class-rejected
Issue-Key: cloudformation__gameliftstreams|CreateStack|ValidationError
Issue-Commit: fce22d92ce0f3b4c883797a9d5b97c4bc60b16e7

# cloudformation__gameliftstreams: AWS::GameLiftStreams::StreamGroup is not a registered resource type — CreateStack refuses every stream-group template as "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `fce22d92ce0f3b4c883797a9d5b97c4bc60b16e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CloudFormation template processor does not know the resource type AWS::GameLiftStreams::StreamGroup, so CreateStack rejects the template at format-validation time and no GameLift Streams stream group can ever be deployed through CloudFormation. Failing checks: cloudformation__gameliftstreams.deploy-gameliftstreams-stream-group-round-trip-and-delete, cloudformation__gameliftstreams.deploy-gameliftstreams-stream-group-invalid-stream-class-rejected.

AWS: AWS::GameLiftStreams::StreamGroup is a supported CloudFormation resource type (https://docs.aws.amazon.com/AWSCloudFormation/latest/TemplateReference/aws-resource-gameliftstreams-streamgroup.html). The service model in this repo carries its CloudFormation resource projection: Smithy CreateStreamGroup (services/service-com-amazonaws-gameliftstreams-gameliftstreams) declares Description (required, @length 1-80), StreamClass (required, enum StreamClass), LocationConfigurations and Tags with cfnMutability traits, and output members Arn/Id plus cfnExcludeProperty on Status/LocationStates/CreatedAt — i.e. the Ref (Arn) and the read-only GetAtt Id the templates use. Real AWS therefore ACCEPTS such a template: CreateStack returns 200 with a StackId, the stack settles CREATE_COMPLETE, Ref resolves to the stream group ARN (arn:<partition>:gameliftstreams:<region>:<account>:streamgroup/<id>) and Fn::GetAtt "Id" to the group id. When a templated property is invalid, AWS does NOT refuse the template; it fails the individual resource and the stack rolls back to ROLLBACK_COMPLETE with a CREATE_FAILED event whose ResourceStatusReason is the registry-handler form `Resource handler returned message: "<service message>" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)` — for StreamClass the inner message is GameLift Streams' own enum validation text, `1 validation error detected: Value '<value>' at 'streamClass' failed to satisfy constraint: Member must satisfy enum value set: [gen4n_high, gen4n_ultra, gen4n_win2022, gen5n_high, gen5n_ultra, gen5n_win2022, gen6n_small, gen6n_medium, gen6n_high, gen6n_ultra, gen6n_ultra_win2022, gen6n_pro, gen6n_pro_win2022, gen6e_pro, gen6e_pro_win2022] (Service: GameLiftStreams, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)` (fixed text is everything but <value>, which comes from the request, and the per-run <id>/<n>). The "Template format error: Unrecognized resource types: [...]" ValidationError is AWS's wording only for a type CloudFormation genuinely does not support.

Emulator: CreateStack is refused outright with HTTP 400, Error Code `ValidationError`, message `Template format error: Unrecognized resource types: [AWS::GameLiftStreams::StreamGroup]` — for both a valid template (StreamClass gen4n_win2022) and the negative-control template (StreamClass not_a_class). No stack is created, so nothing downstream (stack events, Outputs, Ref/GetAtt, ListStreamGroups cross-read, the delete path) is reachable.

Why: a client that provisions GameLift Streams through CloudFormation, CDK or any IaC tool cannot deploy at all — the stack never exists, so the failure is a hard deploy-time refusal rather than a wrong value. It also masks the per-resource rejection path: the caller gets a template-format error instead of a rolled-back stack carrying the service's own validation message, which is what IaC error handling and rollback-detection logic key on.

Fix: the CloudFormation template processor's resource-type registry (the type-name -> resource-handler table consulted during CreateStack template validation) must register AWS::GameLiftStreams::StreamGroup, and a handler must back it: create -> GameLiftStreams CreateStreamGroup (mapping Description, StreamClass, LocationConfigurations, Tags), read -> GetStreamGroup for GetAtt Id and the Ref ARN, delete -> DeleteStreamGroup, with service-side validation errors surfaced as a CREATE_FAILED resource reason in the registry-handler envelope so the stack rolls back instead of the template being refused.

Traces: cloudformation__gameliftstreams.deploy-gameliftstreams-stream-group-round-trip-and-delete (valid template, expects CREATE_COMPLETE, Ref/GetAtt and cross-read via ListStreamGroups/GetStreamGroup, then delete and ResourceNotFoundException); cloudformation__gameliftstreams.deploy-gameliftstreams-stream-group-invalid-stream-class-rejected (invalid StreamClass, expects ROLLBACK_COMPLETE plus the handler reason above).

## Evidence
Expected (AWS): CreateStack with Resources.Sg.Type = "AWS::GameLiftStreams::StreamGroup" is accepted and returns a StackId; the stack reaches CREATE_COMPLETE (valid StreamClass) or ROLLBACK_COMPLETE with a CREATE_FAILED event on logical id "Sg" whose reason is `Resource handler returned message: "1 validation error detected: Value 'not_a_class' at 'streamClass' failed to satisfy constraint: Member must satisfy enum value set: [gen4n_high, ... gen6e_pro_win2022] (Service: GameLiftStreams, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)`.

Actual (emulator): both checks die on the first call —
botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::GameLiftStreams::StreamGroup]

Status 400, Code ValidationError, message fixed text "Template format error: Unrecognized resource types: [" + the template's type names + "]". Diverging part: AWS does not classify this supported type as unrecognized at all — it accepts the template.

Model support that the type is CFN-projected: Smithy CreateStreamGroup input StreamClass (required, enum StreamClass = [gen4n_high, gen4n_ultra, gen4n_win2022, gen5n_high, gen5n_ultra, gen5n_win2022, gen6n_small, gen6n_medium, gen6n_high, gen6n_ultra, gen6n_ultra_win2022, gen6n_pro, gen6n_pro_win2022, gen6n_small_win2022, gen6n_medium_win2022, gen6e_pro, gen6e_pro_win2022]), Tags cfnMutability=full, StreamClass/DefaultApplication cfnMutability=create-and-read, Status/LocationStates/CreatedAt cfnExcludeProperty, Arn as the resource identifier.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. cfn = client("cloudformation"); call CreateStack with StackName=<unique> and TemplateBody = {"AWSTemplateFormatVersion": "2010-09-09", "Resources": {"Sg": {"Type": "AWS::GameLiftStreams::StreamGroup", "Properties": {"Description": "<1-80 chars>", "StreamClass": "gen4n_win2022", "LocationConfigurations": [{"LocationName": "<region>", "AlwaysOnCapacity": 0, "OnDemandCapacity": 0}], "Tags": {"k": "v"}}}}, "Outputs": {"SgArn": {"Value": {"Ref": "Sg"}}, "SgId": {"Value": {"Fn::GetAtt": ["Sg", "Id"]}}}}.
3. Observe ValidationError "Template format error: Unrecognized resource types: [AWS::GameLiftStreams::StreamGroup]" instead of a StackId.
4. Repeat with StreamClass "not_a_class": AWS accepts the template and rolls the stack back with the GameLiftStreams enum-validation handler reason; the emulator returns the same template-format ValidationError.
