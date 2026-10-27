Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-inline-body-openapi-proxy-serves-its-routes
Issue-Key: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-inline-body-openapi-proxy-serves-its-routes|_poll_http#17|Timeout|time.sleep(interval_s)
Issue-Commit: bbbd5f0b3425ac8a27c3dccbe4215ebaac702410

# apigatewayv2__cloudformation: a stack whose AWS::ApiGatewayV2::Api is declared by an inline OpenAPI Body now reaches no terminal state and never serves — the Body-import deploy check hangs to the harness cap (1 cause)

> _Note: this issue was identified against the code base at commit `bbbd5f0b3425ac8a27c3dccbe4215ebaac702410`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause, grounded in one failing behavioral check. Its failure mode has CHANGED from what the open issue 20261026T235900Z-apigatewayv2-cloudformation-2-net-new-root-causes-aws.md recorded (that issue's key for this check is a fast ROLLBACK_COMPLETE assertion inside `_deploy_cfn`, reason `NotImplemented: this emulator does not create a AWS::ApiGatewayV2::Api from its Body property`); the check now produces no verdict at all, so that issue's replay key no longer matches and it does not close this failure.

Cause 1 of 1 — a CloudFormation stack declaring an `AWS::ApiGatewayV2::Api` by its `Body` property (an inline OpenAPI 3.0.1 document whose `/alpha` and `/beta` GET operations each carry an `x-amazon-apigateway-integration` of type AWS_PROXY, payloadFormatVersion 2.0, targeting the stack's own Lambda), together with an IAM role, a python3.12 ZipFile Lambda, a Lambda::Permission and an `AWS::ApiGatewayV2::Stage` ($default, AutoDeploy true), neither converges nor serves: the check is killed at the harness's 60 s cap. Failing check: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-inline-body-openapi-proxy-serves-its-routes.

AWS: recorded behavior only for the CloudFormation Body-import path — the stack reaches CREATE_COMPLETE; GetApi reports `ProtocolType` `HTTP` (inferred, since the template sets no ProtocolType); GetRoutes reports exactly the route keys the document declares (`GET /alpha`, `GET /beta`); GetIntegrations reports ONE integration per path (two here, even though both paths carry an identical integration block), each with `IntegrationType` `AWS_PROXY` and an `IntegrationUri` naming the stack's own function. On the data plane, GET /alpha and GET /beta each answer 200 with the proxied Lambda's body (its `routeKey`/`rawPath` being the route key and raw path of that request), and a path the document never declared answers HTTP 404.

Emulator: no observable outcome at all. The replay verdict is only `Check timed out after 60.0s`: the stack-convergence poll (DescribeStacks until one of CREATE_COMPLETE / CREATE_FAILED / ROLLBACK_COMPLETE / ROLLBACK_FAILED / GONE, budget 600 s) and/or the subsequent bounded invoke-URL poll (budget 180 s) never finish, so the emulator supplies neither a terminal stack status nor a served route — and no longer even the explicit NotImplemented rollback reason it previously gave. A caller is left hanging instead of getting a status or an error it can branch on.

Why: OpenAPI-first / SAM-style deployment (the standard way HTTP APIs are shipped) cannot be emulated, and the failure is now indistinguishable from a hung control plane: a client cannot tell "Body import unsupported" from "stack still creating", gets no CREATE_FAILED/ROLLBACK to react to, and never sees the imported routes or the AWS_PROXY 200s.

Fix: the AWS::ApiGatewayV2::Api provisioner in the cloudformation crate's resource-provisioner layer must drive the apigatewayv2 Body/ImportApi path (registering api, routes and per-path integrations in apigatewayv2 state) and must ALWAYS drive the stack to a terminal state — never leave CreateStack unresolved — and the apigatewayv2 execute-api data plane must then serve the imported routes (200 for declared paths, 404 for undeclared ones).

Traces: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-inline-body-openapi-proxy-serves-its-routes.

## Evidence
Failing check apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-inline-body-openapi-proxy-serves-its-routes — verdict [mismatch], detail: `Error: Check timed out after 60.0s`.

Expected (AWS): CreateStack with {ExecutionRole: AWS::IAM::Role, EchoFunction: AWS::Lambda::Function (python3.12, inline ZipFile), Api: AWS::ApiGatewayV2::Api with only a `Body` (OpenAPI 3.0.1, GET /alpha and GET /beta, each with x-amazon-apigateway-integration type AWS_PROXY / httpMethod POST / payloadFormatVersion 2.0 / uri Fn::Sub of the function ARN), InvokePermission: AWS::Lambda::Permission, Stage: AWS::ApiGatewayV2::Stage ($default, AutoDeploy true)} -> CREATE_COMPLETE; GetApi.ProtocolType == "HTTP"; sorted GetRoutes route keys == ["GET /alpha", "GET /beta"]; GetIntegrations length == 2, every IntegrationType == "AWS_PROXY" and IntegrationUri containing the stack's function name; GET <stage>/alpha -> 200 {"routeKey": "GET /alpha", "rawPath": "/alpha"}; GET <stage>/beta -> 200 {"routeKey": "GET /beta", "rawPath": "/beta"}; GET <stage>/gamma -> 404.

Actual (emulator): no assertion reached — the check was cut off by the 60 s harness cap while polling, so no terminal stack status and no served route were produced. This differs from the behaviour recorded in open issue 20261026T235900Z-apigatewayv2-cloudformation-2-net-new-root-causes-aws.md for the same check (there: a prompt ROLLBACK_COMPLETE with ResourceStatusReason `The following resource(s) failed to create: [Api]. NotImplemented: this emulator does not create a AWS::ApiGatewayV2::Api from its Body property; the resource was not created.`), which is why the emulator's replay key for this check no longer matches that issue.

## Steps to Reproduce
1. CreateStack (Capabilities=[CAPABILITY_IAM, CAPABILITY_NAMED_IAM]) with the template above: AWS::IAM::Role, AWS::Lambda::Function (python3.12, Code.ZipFile inline echo handler), AWS::ApiGatewayV2::Api whose ONLY property is `Body` (inline OpenAPI 3.0.1 with GET /alpha and GET /beta, each carrying an identical x-amazon-apigateway-integration of type AWS_PROXY, payloadFormatVersion 2.0, uri = Fn::Sub of the function ARN), AWS::Lambda::Permission (Principal apigateway.amazonaws.com, SourceArn Fn::Sub execute-api ${Api}/*/*), AWS::ApiGatewayV2::Stage ($default, AutoDeploy true), and Output ApiId = Ref Api.
2. Poll DescribeStacks for a terminal create state: AWS reaches CREATE_COMPLETE; the emulator returns no terminal state within the poll window (and previously returned ROLLBACK_COMPLETE with a NotImplemented reason for the Body property).
3. GetApi(ApiId): expect ProtocolType HTTP. GetRoutes(ApiId): expect exactly `GET /alpha` and `GET /beta`. GetIntegrations(ApiId): expect 2 AWS_PROXY integrations whose IntegrationUri names the stack's function.
4. GET <$default stage invoke URL>/alpha and /beta: expect 200 with {"routeKey": ..., "rawPath": ...} from the echo Lambda; GET /gamma: expect 404.
