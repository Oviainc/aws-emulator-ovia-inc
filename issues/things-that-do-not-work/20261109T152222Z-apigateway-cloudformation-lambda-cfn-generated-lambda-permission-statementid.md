Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__lambda.deploy-cdk-lambdarestapi-deploy-false-test-invoke-returns-function-response apigateway__cloudformation__lambda.cfn-custom-resource-signals-failure-stack-rollback-negative apigateway__cloudformation__lambda.deploy-cfn-apigw-lambda-integration-missing-permission-test-invoke-rejected
Issue-Key: apigateway__cloudformation__lambda.deploy-cdk-lambdarestapi-deploy-false-test-invoke-returns-function-response|_run_cli#15|AssertionError|assert result.returncode == 0, (
Issue-Key: apigateway__cloudformation__lambda.cfn-custom-resource-signals-failure-stack-rollback-negative|run#13|AssertionError|assert last == "ROLLBACK_FAILED", f"got={last}"
Issue-Key: apigateway__cloudformation__lambda.deploy-cfn-apigw-lambda-integration-missing-permission-test-invoke-rejected|run#26|AssertionError|assert denied["status"] == 500, (
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigateway__cloudformation__lambda: CFN-generated Lambda::Permission StatementId exceeds 100 chars, a never-answered custom resource settles ROLLBACK_COMPLETE, and TestInvokeMethod ignores the missing invoke permission (3 causes)

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddPermission

## Description
Three net-new root causes in the CloudFormation-provisioned API Gateway + Lambda path. Numbered; each carries its own failing check ids.

Cause 1 of 3: the AWS::Lambda::Permission handler derives a StatementId longer than Lambda's cap, so any CDK/SAM stack with a generated permission logical id rolls back.
AWS: lambda AddPermission's StatementId has a maximum length of 100. Real CloudFormation therefore derives the permission's StatementId so that it fits — a stack whose AWS::Lambda::Permission logical id is long (as CDK's generated ids routinely are) still creates successfully.
Emulator: the CFN handler concatenates the stack name, the full logical id and a hex suffix into the StatementId and passes it straight to AddPermission, which refuses with 400 "1 validation error detected: Value '<stackName>-<logicalId>-<suffix>' at 'statementId' failed to satisfy constraint: Member must have length less than or equal to 100". The resource reports CREATE_FAILED with HandlerErrorCode InvalidRequest and the whole stack rolls back, so `cdk deploy` exits 1.
Why: no CDK app that grants API Gateway permission to invoke a Lambda (the LambdaRestApi / addMethod default) can be deployed at all. The failing value is a template the handler builds, not one the user supplied.
Fix: the CloudFormation AWS::Lambda::Permission resource handler's StatementId derivation (it must produce a value within Lambda's 100-character limit).
Traces: apigateway__cloudformation__lambda.deploy-cdk-lambdarestapi-deploy-false-test-invoke-returns-function-response

Cause 2 of 3: a Lambda-backed custom resource that never PUTs a response settles the stack at ROLLBACK_COMPLETE instead of ROLLBACK_FAILED.
AWS: when a custom resource's function faults before signalling (here it raises AccessDenied on ssm:GetParameter), CloudFormation records the resource CREATE_FAILED with the fixed reason "CloudFormation did not receive a response from your Custom Resource. Please check your logs for requestId [<requestId>]..." and rolls back; the rollback's own Delete signal is blocked by the same missing permission and also goes unanswered, so the stack lands in ROLLBACK_FAILED — not ROLLBACK_COMPLETE.
Emulator: the stack settles at ROLLBACK_COMPLETE, i.e. the rollback is reported as having succeeded even though the custom resource was never rolled back.
Why: a client polling DescribeStacks sees a clean rollback and believes the stack is back to a consistent state, when AWS reports a stuck rollback that needs operator attention (ContinueUpdateRollback / manual cleanup).
Fix: the CloudFormation custom-resource rollback path — an unanswered Delete signal during rollback must leave the stack ROLLBACK_FAILED.
Traces: apigateway__cloudformation__lambda.cfn-custom-resource-signals-failure-stack-rollback-negative

Cause 3 of 3: TestInvokeMethod succeeds for an AWS_PROXY method with no AWS::Lambda::Permission.
AWS: an AWS_PROXY integration whose function carries no resource policy statement allowing apigateway.amazonaws.com to invoke it fails with a configuration error; TestInvokeMethod answers status 500 and its execution log carries the configuration-error line ("Execution failed due to configuration error: Invalid permissions on Lambda function") and "Method completed with status: 500".
Emulator: TestInvokeMethod answers status 200 with the function's body, and the log reads "Integration type: AWS_PROXY / Successfully completed execution / Method completed with status: 200" — the permission is never consulted.
Why: the negative permission contract passes vacuously; a client cannot detect a missing lambda:InvokeFunction grant before shipping, and an over-permissive emulator hides a production 500.
Fix: the execute-api / TestInvokeMethod integration path's Lambda resource-policy check before invoking an AWS_PROXY function.
Traces: apigateway__cloudformation__lambda.deploy-cfn-apigw-lambda-integration-missing-permission-test-invoke-rejected

## Evidence
Cause 1: "`cdk deploy sim2real-lrap-...` exited 1, expected 0" with "Resource handler returned message: \"1 validation error detected: Value '<stack>-ApiproxyANYApiPermission...-4D834CDB5B75' at 'statementId' failed to satisfy constraint: Member must have length less than or equal to 100 (Service: Lambda, Status Code: 400 ...)\" (HandlerErrorCode: InvalidRequest)" on AWS::Lambda::Permission.
Cause 2: "assert last == \"ROLLBACK_FAILED\", f\"got={last}\" ... AssertionError: got=ROLLBACK_COMPLETE".
Cause 3: "POST /denied through a method with no Lambda::Permission returned status 200, expected 500", with the log containing "Integration type: AWS_PROXY / Successfully completed execution / Method completed with status: 200".

## Steps to Reproduce
1. Cause 1: `cdk deploy` a stack using LambdaRestApi (or any AWS::Lambda::Permission whose logical id is long enough that stackName+logicalId+suffix exceeds 100 chars). Expect CREATE_COMPLETE.
2. Cause 2: create a Lambda whose role lacks ssm:GetParameter, use it as the ServiceToken of a Custom:: resource that calls ssm:GetParameter before signalling, create the stack, and poll DescribeStacks. Expect ROLLBACK_FAILED and a Signal CREATE_FAILED event whose reason is "CloudFormation did not receive a response from your Custom Resource. Please check your logs for requestId [...]".
3. Cause 3: deploy a CFN stack with an AWS_PROXY method and NO AWS::Lambda::Permission, then TestInvokeMethod on that method. Expect status 500 and the configuration-error log line.
