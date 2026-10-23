Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-websocket-route-selection-expression-dispatches-by-action apigatewayv2__cloudformation.stage-auto-deploy-republishes-on-a-cfn-route-change
Issue-Key: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-websocket-route-selection-expression-dispatches-by-action|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__cloudformation.stage-auto-deploy-republishes-on-a-cfn-route-change|_poll_http#12|Timeout|time.sleep(interval_s)
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# apigatewayv2__cloudformation: stacks containing AWS::ApiGatewayV2::Stage/Route never converge — both deploy checks hang to the harness cap (1 cause)

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStage

## Description
Cause 1 of 1: a CloudFormation stack whose template contains AWS::ApiGatewayV2::Stage and AWS::ApiGatewayV2::Route resources never reaches a terminal stack state, so both checks that deploy one hang and are killed by the harness at 60s with no stack outcome to assert on.
AWS: Smithy model (services/service-com-amazonaws-apigatewayv2-apigatewayv2/smithy-model) CreateStage -> POST /v2/apis/{ApiId}/stages with success code 201 and modelled errors BadRequestException (400), ConflictException (409), NotFoundException (404), TooManyRequestsException (429); CloudFormation provisions AWS::ApiGatewayV2::Stage through it. Recorded behavior: CreateStack for these templates must reach a terminal state (CREATE_COMPLETE, or a terminal failure state carrying the underlying API error) within the check's budget, after which the WebSocket API dispatches by RouteSelectionExpression $request.body.action, and AutoDeploy republishes a new DeploymentId on the $default stage when a route is added by a stack update while a stage pinned to an older deployment keeps answering 404.
Emulator: both checks report only 'Check timed out after 60.0s' — no stack status, no error envelope. Hypothesis for the mechanism: the underlying apigatewayv2 CreateStage operation answers the off-model error code 'NotImplemented' ('operation CreateStage is scaffolded but not implemented in this emulator') — observed directly in this same replay from apigatewayv2__lambda, apigatewayv2__cloudwatch, apigatewayv2__dynamodb__lambda and cloudwatch__apigatewayv2 — and the stack neither completes nor fails terminally when its resource provisioner receives that, leaving the create in a non-terminal state that the checks poll until the cap. Only these two checks and the four apigatewayv2__cognito-idp__lambda JWT checks time out in the whole 225-check suite, so this is not a blanket harness budget effect.
Why: a client that deploys an API Gateway v2 stack gets neither a usable API nor a failure it can react to — CreateStack simply never settles, so IaC-driven provisioning, rollback and stack-update flows all stall instead of erroring.
Fix: the CreateStage handler and stage state (services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_stage.rs) plus the CloudFormation provisioner for AWS::ApiGatewayV2::Stage/Route, which must drive the stack to a terminal state (and surface the resource's error) rather than stalling.
Note: the open issue 20261023T125319Z-apigatewayv2-cloudformation-deployed-api-gateway-v2-stacks-are-unusable.md covers Ref of AWS::ApiGatewayV2::Api and over-long generated IAM role names and owns other checks, but carries neither replay key below and does not describe a non-terminating stack create.
Traces: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-websocket-route-selection-expression-dispatches-by-action, apigatewayv2__cloudformation.stage-auto-deploy-republishes-on-a-cfn-route-change.

## Evidence
Replay of suite simparity-behavioral-aws-v1: apigatewayv2__cloudformation 0/6 (0%), 6 failed; the two checks below are the ones not already owned by an open issue and both report:
Error: Check timed out after 60.0s

Expected: CreateStack of a template with AWS::ApiGatewayV2::Api + Integration + Route + Stage reaches a terminal stack state within the budget, then the WebSocket API dispatches 'ping'/'sendMessage'/$default by RouteSelectionExpression, and AutoDeploy yields a new DeploymentId on $default after a stack update adds a route.
Actual: no terminal stack state and no error envelope within 60s; the checks' assertions were never evaluated.

Corroborating same-replay evidence that the underlying operation refuses: 'An error occurred (NotImplemented) when calling the CreateStage operation: operation CreateStage is scaffolded but not implemented in this emulator' from apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-lambda-handler-raises-rejected-500 and apigatewayv2__cloudwatch.http-api-count-and-4xx-metrics-carry-the-apiid-stage-dimensions.

Neither check carried an 'ALREADY FILED' or 'OWNED by' marker.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateStack with a template containing AWS::ApiGatewayV2::Api (ProtocolType WEBSOCKET, RouteSelectionExpression '$request.body.action'), an AWS_PROXY integration to an inline Lambda, AWS::ApiGatewayV2::Route resources, and an AWS::ApiGatewayV2::Stage.
3. Poll DescribeStacks. Expect a terminal StackStatus (CREATE_COMPLETE, or a terminal failure naming the failing resource and its error). Observe that the stack does not settle and the client blocks indefinitely.
4. Independently call apigatewayv2 CreateStage(ApiId=<api>, StageName='p') and observe error code NotImplemented, which the stack provisioner must either implement or surface terminally.
5. Re-replay the two check ids in trace_ids.
