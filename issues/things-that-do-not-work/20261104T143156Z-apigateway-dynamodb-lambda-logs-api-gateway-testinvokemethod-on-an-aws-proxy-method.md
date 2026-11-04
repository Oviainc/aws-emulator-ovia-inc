Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.cli-apigateway-missing-lambda-permission-rejected
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-missing-lambda-permission-rejected|_wait_for_permission_effective#33|Timeout|return last
Issue-Commit: 732d86be3641565fdf04b49c6a1d1231e9cf6723

# API Gateway: TestInvokeMethod on an AWS_PROXY method never produces the backend outcome, so the missing-lambda-permission check never completes

> _Note: this issue was identified against the code base at commit `732d86be3641565fdf04b49c6a1d1231e9cf6723`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
One net-new root cause, grounded in the single failing check listed under `### apigateway__dynamodb__lambda__logs` with no ALREADY FILED / OWNED marker (the other 82 failures of that heading are all owned). Cause count: 1 — it must be closed before this issue is submitted.

RELATED OPEN ISSUE (it does not carry this check's replay key): 20261102T090835Z-apigateway-dynamodb-lambda-logs-api-gateway-rest-proxy-integrations-never-reach-a.md — 'REST proxy integrations never reach a Lambda'. The cause below is the same family observed through apigateway TestInvokeMethod rather than a stage URL; consolidate if the fix is one and the same, but this check id is new and must be re-replayed to close it.

=== Cause 1 of 1: a REST method with an AWS_PROXY integration yields neither the permission-denied outcome nor, after the grant, the function's own response ===
AWS: for a GET method whose integration is type AWS_PROXY pointing at arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fnArn>/invocations, apigateway TestInvokeMethod answers immediately with the backend outcome: while no lambda:InvokeFunction resource-policy statement grants apigateway.amazonaws.com on that function, `status` is 500 and the body does not contain anything the function produced, and the function does NOT execute (no new event in /aws/lambda/<fn>); once lambda AddPermission(Action='lambda:InvokeFunction', Principal='apigateway.amazonaws.com', SourceArn='arn:aws:execute-api:<region>:<acct>:<apiId>/*/GET/echo') is added, the identical call answers `status` 200 with the function's body and its stdout appears in the function's log group. Recorded behavior only for the two status codes; the Smithy model expresses neither the authorization rule nor the latency.
Emulator: the check makes no observable progress and the harness kills it at its 60s cap (verdict 'mismatch', 'Check timed out after 60.0s'). All of its setup is ordinary 2xx control-plane work (iam CreateRole/AttachRolePolicy, lambda CreateFunction via the CLI, apigateway CreateRestApi/CreateResource/PutMethod/PutIntegration/CreateDeployment); the only unbounded waits in it are the negative arm's 20s proof that the function did NOT run, then the poll for TestInvokeMethod to return 200 after the grant, then a 60s poll for the function's log marker. Hypothesis: the AWS_PROXY method never actually reaches the function, so the post-grant 200 and/or the execution log never arrive and the 60s marker poll runs to its full length, pushing the check past the budget. What is certain is that the operation sequence that completes on AWS in a few seconds does not complete here at all.
Why: the permission boundary of a proxy integration is untestable and a client cannot tell 'denied' from 'the gateway never invoked anything'; worse, the positive control does not come back either, so an AWS_PROXY REST method appears permanently non-functional.
Fix: the TestInvokeMethod handler for AWS_PROXY integrations in the apigateway service crate — it must resolve the integration URI to the function, consult that function's resource policy (answering 500 without a matching lambda:InvokeFunction statement for apigateway.amazonaws.com and the method's execute-api source ARN, with no invocation), and on success return the function's status/body and leave its execution record in the function's log group.
Traces: apigateway__dynamodb__lambda__logs.cli-apigateway-missing-lambda-permission-rejected.

## Evidence
[mismatch] apigateway__dynamodb__lambda__logs.cli-apigateway-missing-lambda-permission-rejected
  Error: Check timed out after 60.0s

The check's assertions, none of which was reached conclusively: `assert denied_result["status"] == 500` for `aws apigateway test-invoke-method --rest-api-id <id> --resource-id <id> --http-method GET` before any grant; the marker must be absent from /aws/lambda/<fn> for 20s ('lambda was actually executed despite missing invoke permission'); then after `aws lambda add-permission --action lambda:InvokeFunction --principal apigateway.amazonaws.com --source-arn arn:aws:execute-api:<region>:<acct>:<apiId>/*/GET/echo`, `assert allowed_result["status"] == 200` for the identical test-invoke-method call, the marker present in the body, and the function's stdout line present in its log group within 60s. Expected: both calls answer promptly with 500 then 200; observed: the check never finishes inside the 60s budget. (The api id, function name, account and region above are this run's values filling the templates.)

## Steps to Reproduce
1. iam.create_role(trusting lambda.amazonaws.com) + attach AWSLambdaBasicExecutionRole; lambda.create_function(python handler printing a marker and returning {'statusCode':200,'body':...}); wait for State=Active.
2. apigateway.create_rest_api; get_resources -> root id; create_resource(pathPart='echo'); put_method(httpMethod='GET', authorizationType='NONE'); put_integration(type='AWS_PROXY', integrationHttpMethod='POST', uri='arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fnArn>/invocations'); create_deployment(stageName='test').
3. apigateway.test_invoke_method(restApiId=<id>, resourceId=<id>, httpMethod='GET') with NO lambda resource policy -> EXPECT status 500, body without the function's marker, and no event in /aws/lambda/<fn>.
4. lambda.add_permission(FunctionName=<fn>, StatementId='apigw-invoke', Action='lambda:InvokeFunction', Principal='apigateway.amazonaws.com', SourceArn='arn:aws:execute-api:<region>:<acct>:<apiId>/*/GET/echo').
5. Repeat step 3 -> EXPECT status 200, the function's marker in the body, and the function's stdout line in /aws/lambda/<fn>. OBSERVE: the sequence does not complete; the check is killed after 60s.
