Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-stripped-lambda-permission-invoke-rejected
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-stripped-lambda-permission-invoke-rejected|_http_until#18|Timeout|time.sleep(3.0)
Issue-Commit: 440ede2c1b46a54e9cd3a5bcdc414c48552fbe21

# apigatewayv2: a CDK HttpApi whose AWS::Lambda::Permission is stripped never reaches a verdict — the AWS_PROXY route's resource-policy gate and the UpdateStack repair back to 200 are both unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `440ede2c1b46a54e9cd3a5bcdc414c48552fbe21`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api data plane, AWS_PROXY integration)

## Description
Cause 1 of 1: the permission-gated leg of an HTTP API's AWS_PROXY ($default) route produces no observable behavior at all, so the check that strips exactly one AWS::Lambda::Permission from a CDK-synthesised HttpApi stack and then restores it is killed at the suite's 60 s cap with no verdict. Failing check: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-stripped-lambda-permission-invoke-rejected.

AWS: recorded behavior only for the data-plane half — the apigatewayv2 Smithy model (services/service-com-amazonaws-apigatewayv2-apigatewayv2/smithy-model/com-amazonaws-apigatewayv2-apigatewayv2.json) binds no execute-api invoke operation; the only modeled observations the check makes are GetRoutes (GET /v2/apis/{ApiId}/routes) and GetIntegrations (GET /v2/apis/{ApiId}/integrations), which must report exactly one route `$default` and exactly one integration of IntegrationType AWS_PROXY for the deployed stack. The recorded behavior of the data plane is:
  (a) With no resource policy on the integration's function (lambda GetPolicy raises ResourceNotFoundException, error code `ResourceNotFoundException`), an unsigned GET on the stage endpoint is refused with HTTP 500 and body exactly {"message": "Internal Server Error"} — the fixed text of that body is the whole requirement, it carries no request- or state-derived part — and the function is NOT invoked, so nothing the handler would have produced (the run-unique marker the handler echoes) appears in the response. The refusal is stable: three consecutive GETs all answer 500 with that body.
  (b) The function itself is healthy on that same path: lambda Invoke (RequestResponse) on it returns a payload with no FunctionError whose `body` is the handler's JSON object — keys `marker`, `path`, `method`, with `marker` the value passed to the function's environment/inline code, `path` the event's rawPath and `method` the event's HTTP method. So the 500 is attributable to the missing permission alone.
  (c) Restoring ONLY the AWS::Lambda::Permission resource, by UpdateStack with the unmodified synthesised template, reaches UPDATE_COMPLETE, and lambda GetPolicy then returns a policy with exactly one Statement whose Principal is {"Service": "apigateway.amazonaws.com"} and whose Condition.ArnLike."AWS:SourceArn" ends with `:<the deployed ApiId>/*/*` (the api id comes from the stack's ApiId output; the `/*/*` suffix is the fixed part).
  (d) After that single repair the same unsigned GET answers HTTP 200 with the handler's JSON body (the marker from the request/stack, path "/", method "GET") — no redeploy of any other resource.

Emulator: the check never gets far enough for any of (a)-(d) to be decided; the replay detail is `Error: Check timed out after 60.0s`, i.e. one of its bounded polls (_wait_stack for CREATE_COMPLETE/UPDATE_COMPLETE, or _http_until on the stage endpoint) never converged. On this build the pieces that leg needs are missing or inert: CloudFormation provisions no AWS::Lambda::Function / AWS::Lambda::Permission resource (so the stack publishes a FunctionName output for a function the lambda service does not hold, GetPolicy answers ResourceNotFoundException for the wrong reason — the function does not exist rather than has no policy — and the UpdateStack repair can never produce the restored policy of (c)), and lambda Invoke cannot execute a deployment package, so the AWS_PROXY route has no backend to invoke and the positive control (b) and the restored 200 of (d) are unreachable. The result is that the emulator cannot distinguish "refused because API Gateway found no invoke permission" from "500 because no integration response was ever produced": the 500 of (a) is emitted, if at all, for a reason unrelated to the resource policy, so the negative is not actually enforced.

Why: this is the standard shape of a Lambda-backed HTTP API shipped by CDK/CloudFormation, and the resource-policy gate is the security boundary of it. A client relying on AWS's behavior gets two wrong answers: a route whose function grants API Gateway nothing is not demonstrably refused (an emulator that invokes anyway would leak the handler's response to an unauthorized caller, and here the refusal that is produced is indistinguishable from a generic backend failure), and removing or restoring an AWS::Lambda::Permission through a stack update has no observable effect on whether the route serves — so a deployment whose permission was dropped looks identical to a healthy one, and the repair cannot be verified.

Fix: three components on one path — (1) the CloudFormation provisioner for AWS::Lambda::Function and AWS::Lambda::Permission, so the synthesised stack really creates the function (packaging inline Code.ZipFile) and its resource policy, and an UpdateStack that re-adds the Permission really calls AddPermission; (2) the lambda invoke path, so a deployment package can run and the AWS_PROXY integration has a real backend; (3) the apigatewayv2 data-plane AWS_PROXY dispatch in services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/service/invoke.rs, which must evaluate the target function's resource policy against the execute-api ARN of the MATCHED route before invoking, and answer 500 {"message": "Internal Server Error"} without invoking when nothing admits apigateway.amazonaws.com. The open issue 20261027T192238Z-apigatewayv2-lambda-cloudformation-a-cdk-synthesised-httpapi-stack-cannot-serve.md describes the shared provisioning/runtime prerequisites for the sibling positive check (deploy-apigatewayv2-3-cdk-http-api-cors-lambda-default-route-serves-then-destroys) but does NOT carry this check's replay key and says nothing about the resource-policy gate or the Permission strip/restore round-trip, which is the behavior asserted here.

Traces: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-stripped-lambda-permission-invoke-rejected.

## Evidence
Failing check apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-stripped-lambda-permission-invoke-rejected — verdict [mismatch], detail: `Error: Check timed out after 60.0s` (no assertion was ever reached; the check's own bounds are 300 s for stack create/update and 90 s for each endpoint poll).

Expected (AWS):
- GetRoutes -> exactly ["$default"]; GetIntegrations -> exactly one item with IntegrationType "AWS_PROXY".
- lambda GetPolicy on the stripped function -> ClientError code "ResourceNotFoundException".
- Unsigned GET <ApiEndpoint>/ -> HTTP 500, body exactly {"message": "Internal Server Error"}, repeated three times, with the handler's marker absent from the body.
- lambda Invoke on that function -> no FunctionError, payload body == {"marker": <marker>, "path": "/", "method": "GET"} (values from the request; the keys are the fixed shape).
- UpdateStack with the Permission restored -> UPDATE_COMPLETE; GetPolicy -> exactly one Statement, Principal {"Service": "apigateway.amazonaws.com"}, Condition.ArnLike["AWS:SourceArn"] ending ":<ApiId>/*/*".
- Unsigned GET <ApiEndpoint>/ -> HTTP 200 with that same handler body.

Actual (emulator): no terminal result. The permission-gated refusal and the post-repair 200 are both unobservable, because the stack creates no AWS::Lambda::Function/Permission and the AWS_PROXY route has no function to invoke or policy to evaluate; the data plane's 500, where it appears, is the generic "integration produced no response" answer rather than a resource-policy refusal, so the negative is not enforced and the restore cannot be verified.

## Steps to Reproduce
1. `cdk synth` (LegacyStackSynthesizer, no bootstrap) an app with: an inline-code python3.12 Lambda, an explicit log group, and an apigatewayv2.HttpApi whose default_integration is HttpLambdaIntegration(fn) with a CORS preflight config. Confirm the template contains exactly one AWS::Lambda::Permission.
2. Delete that one AWS::Lambda::Permission from the template and CreateStack it (Capabilities=[CAPABILITY_IAM]); poll DescribeStacks to CREATE_COMPLETE and read the ApiId / ApiEndpoint / FunctionName outputs.
3. GetRoutes/GetIntegrations on the ApiId: expect ["$default"] and one AWS_PROXY integration.
4. lambda GetPolicy on FunctionName: expect ResourceNotFoundException (no resource policy at all).
5. Unsigned GET on <ApiEndpoint>/: expect HTTP 500 with body exactly {"message": "Internal Server Error"} on three consecutive attempts, and the handler's marker absent.
6. lambda Invoke on FunctionName with {"rawPath": "/", "requestContext": {"http": {"method": "GET"}}}: expect no FunctionError and the handler's JSON body.
7. UpdateStack with the UNSTRIPPED template; wait UPDATE_COMPLETE; GetPolicy: expect one statement, Principal {"Service": "apigateway.amazonaws.com"}, SourceArn ending ":<ApiId>/*/*".
8. Unsigned GET on <ApiEndpoint>/ again: expect HTTP 200 with the handler's body.
9. Replay apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-stripped-lambda-permission-invoke-rejected.
