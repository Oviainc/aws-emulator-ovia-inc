Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudformation__lambda.sam-httpapi-event-serves-payload-v2-route
Issue-Key: apigatewayv2__cloudformation__lambda.sam-httpapi-event-serves-payload-v2-route|_create_stack#9|AssertionError|assert status == "CREATE_COMPLETE", f"stack {name} ended {status}: {_failures(cfn, stack_id)!r}"
Issue-Commit: e5a3e8eb74dcb2b43ac4e38f0bb7bcf8e9a5cd29

# apigatewayv2__cloudformation__lambda: a SAM AWS::Serverless::HttpApi/HttpApi-event stack rolls back — the AUTO_EXPAND transform never supplies the implicit API's Name, so CreateApi is refused "Value null at 'name'" (1 cause)

> _Note: this issue was identified against the code base at commit `e5a3e8eb74dcb2b43ac4e38f0bb7bcf8e9a5cd29`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a CloudFormation stack that uses the AWS::Serverless transform (CAPABILITY_AUTO_EXPAND) and declares a Lambda function with an `HttpApi` event cannot be created. The transform's implicit HTTP API resource is submitted to apigatewayv2 CreateApi with no Name, the CreateApi handler refuses it, and the whole stack reaches ROLLBACK_COMPLETE.

AWS: Smithy (services/service-com-amazonaws-apigatewayv2-apigatewayv2/smithy-model/com-amazonaws-apigatewayv2-apigatewayv2.json) models CreateApi POST /v2/apis with success code 201 and `Name` (jsonName `name`, target StringWithLengthBetween1And128) as a REQUIRED member — so a CreateApi call with a null name is indeed invalid, and the refusal itself is correct. The defect is upstream of it: on real AWS, a SAM template whose function carries an `HttpApi` event (plus Globals.Function MemorySize and Globals.HttpApi CorsConfiguration) is expanded by the AWS::Serverless transform into a concrete AWS::ApiGatewayV2::Api resource that ALWAYS carries a derived name (the implicit API is named from the stack/logical id, e.g. `ServerlessHttpApi`), together with its $default stage, routes, integrations and the lambda invoke permission. The stack therefore reaches CREATE_COMPLETE, GetFunctionConfiguration reports the Globals MemorySize (192 here), the stack's Endpoint output serves GET /hello with a 2.0 payload event, the Globals CORS answers only the allowed Origin, and an undeclared path is a 404. Recorded behavior for the failure itself: the Api resource's handler error is `Resource handler returned message: "1 validation error detected: Value null at 'name' failed to satisfy constraint: Member must not be null"` with HandlerErrorCode `InvalidRequest`, and the stack-level reason `The following resource(s) failed to create: [Api].` The fixed text there is the validation-error sentence and the HandlerErrorCode; the RequestToken is a per-run value and no part of the requirement.
Emulator: CreateStack with CAPABILITY_IAM + CAPABILITY_AUTO_EXPAND on the SAM HttpApi template transitions the stack to ROLLBACK_COMPLETE. The single failing resource is the implicit `Api`, failing with the null-`name` validation error above — the transform emitted an AWS::ApiGatewayV2::Api with no Name property at all. Because the stack rolls back, nothing the scenario depends on exists: no function MemorySize to read back, no stack Outputs (so no Endpoint), no $default stage, no route, no CORS behavior and no 404 contrast.
Why: the whole SAM HttpApi authoring path — the single most common way customers declare an HTTP API in CloudFormation — is unusable against the emulator. A client gets a rolled-back stack with an error that blames a null name it never wrote, so the failure is not even diagnosable from the template: it points at a property the user does not control. Everything layered on an implicit HTTP API (Globals.Function propagation, Globals.HttpApi CORS, the $default stage endpoint, payload-format-2.0 events) is unreachable.
Fix: the AWS::Serverless transform/macro expansion in the cloudformation service crate — the implicit HTTP API synthesized for a function's `HttpApi` event must be emitted with a derived Name (and its $default stage, route, AWS_PROXY integration and lambda permission), so the AWS::ApiGatewayV2::Api resource handler is called with a non-null `name`. The apigatewayv2 CreateApi required-member validation that reports the error is behaving correctly and should not be loosened.
Traces: apigatewayv2__cloudformation__lambda.sam-httpapi-event-serves-payload-v2-route.

## Evidence
Expected: CreateStack(TemplateBody=<SAM template with Transform: AWS::Serverless-2016-10-31, a function carrying an HttpApi event, Globals.Function.MemorySize=192 and Globals.HttpApi.CorsConfiguration>, Capabilities=[CAPABILITY_IAM, CAPABILITY_AUTO_EXPAND]) reaches CREATE_COMPLETE, exposes an `Endpoint` output, and GET <Endpoint>/hello answers 200 with a 2.0-format event.
Actual (apigatewayv2__cloudformation__lambda.sam-httpapi-event-serves-payload-v2-route, check line 41 -> _create_stack line 124): `AssertionError: stack sim2real-sam-http-2ef48cc5ffd4 ended ROLLBACK_COMPLETE: [('sim2real-sam-http-2ef48cc5ffd4', 'The following resource(s) failed to create: [Api]. '), ('Api', 'Resource handler returned message: "1 validation error detected: Value null at \'name\' failed to satisfy constraint: Member must not be null" (RequestToken: b8a27a6f-06e3-7e2a-4ea6-d79973b90333, HandlerErrorCode: InvalidRequest)')]`.
The stack name `sim2real-sam-http-2ef48cc5ffd4` and the RequestToken are per-run values and are not part of the requirement; the requirement is that no such failure occurs at all, because the transform supplies the implicit API's name.
Smithy citation: CreateApi.Name — target StringWithLengthBetween1And128, jsonName `name`, required; CreateApi http {method: POST, uri: /v2/apis, code: 201}; errors BadRequestException(400), ConflictException(409), NotFoundException(404), TooManyRequestsException(429).

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Take a SAM template that declares `Transform: AWS::Serverless-2016-10-31`, `Globals: Function: {MemorySize: 192}` and `Globals: HttpApi: {CorsConfiguration: {AllowOrigins: [<origin>]}}`, and one AWS::Serverless::Function with an `Events: { Api: { Type: HttpApi, Properties: { Path: /hello, Method: get } } }` event, plus an `Endpoint` output for the implicit API.
3. cloudformation CreateStack(StackName=<s>, TemplateBody=<that template>, Parameters=[FunctionName, AllowedOrigin], Capabilities=['CAPABILITY_IAM','CAPABILITY_AUTO_EXPAND']).
4. Poll DescribeStacks: the stack ends ROLLBACK_COMPLETE; DescribeStackEvents shows resource `Api` failing with `1 validation error detected: Value null at 'name' failed to satisfy constraint: Member must not be null` (HandlerErrorCode: InvalidRequest).
5. Replay apigatewayv2__cloudformation__lambda.sam-httpapi-event-serves-payload-v2-route.
