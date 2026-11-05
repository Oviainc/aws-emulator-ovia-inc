Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__lambda.lambda-sdk-apigateway-create-deployment-missing-permission-negative apigateway__cloudformation__lambda.lambda-sdk-apigateway-create-deployment-positive apigateway__cloudformation__lambda.lambda-sdk-apigateway-get-rest-apis-missing-permission-negative apigateway__cloudformation__lambda.lambda-sdk-apigateway-get-rest-apis-positive apigateway__cloudformation__lambda.lambda-sdk-cfn-get-template-positive apigateway__cloudformation__lambda.lambda-sdk-cfn-list-stack-resources-missing-permission-negative apigateway__cloudformation__lambda.lambda-sdk-cfn-list-stack-resources-positive apigateway__cloudformation__lambda.cfn-lambda-role-trust-policy-positive
Issue-Key: apigateway__cloudformation__lambda.lambda-sdk-apigateway-create-deployment-missing-permission-negative|_invoke#7|AssertionError|raise AssertionError(f"lambda invocation failed: {body}")
Issue-Key: apigateway__cloudformation__lambda.lambda-sdk-apigateway-create-deployment-positive|_invoke#5|AssertionError|raise AssertionError(f"lambda invocation failed: {body}")
Issue-Key: apigateway__cloudformation__lambda.lambda-sdk-apigateway-get-rest-apis-missing-permission-negative|_invoke#9|AssertionError|raise AssertionError(f"lambda invocation failed: {body}")
Issue-Key: apigateway__cloudformation__lambda.lambda-sdk-apigateway-get-rest-apis-positive|_invoke#7|AssertionError|raise AssertionError(f"lambda invocation failed: {body}")
Issue-Key: apigateway__cloudformation__lambda.lambda-sdk-cfn-get-template-positive|_invoke#4|AssertionError|raise AssertionError(f"lambda invocation failed: {body}")
Issue-Key: apigateway__cloudformation__lambda.lambda-sdk-cfn-list-stack-resources-missing-permission-negative|_invoke#7|AssertionError|raise AssertionError(f"lambda invocation failed: {body}")
Issue-Key: apigateway__cloudformation__lambda.lambda-sdk-cfn-list-stack-resources-positive|_invoke#4|AssertionError|raise AssertionError(f"lambda invocation failed: {body}")
Issue-Key: apigateway__cloudformation__lambda.cfn-lambda-role-trust-policy-positive|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# apigateway__cloudformation__lambda: the Lambda python runtime has no AWS SDK, and the CFN Lambda-role trust check never completes

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 2 — the managed python runtime ships no boto3/botocore.
AWS: the python3.x managed runtimes bundle boto3 and botocore on sys.path, so `import boto3` in a handler succeeds and the handler can call API Gateway and CloudFormation with its execution role.
Emulator: HTTP 200 with FunctionError=Unhandled and payload {"errorMessage":"Unable to import module 'index': No module named 'boto3'","errorType":"Runtime.ImportModuleError","stackTrace":[]} — the fixed text is "Unable to import module '<module>': No module named 'boto3'" with the handler's module name substituted.
Why: no handler can call apigateway:CreateDeployment/GetRestApis or cloudformation:GetTemplate/ListStackResources, so both the positive paths and the IAM-denial controls are unobservable; a caller that relies on the managed SDK gets an init failure instead of a result.
Fix: the python runtime's bundled site-packages / module search path in the lambda runtime component. This cause is filed per service; related open issues exist for other service groups (e.g. 20261105T085348Z-appconfig-iam-lambda-the-in-process-lambda-python.md) but do not carry these replay keys.
Traces: the seven lambda-sdk-* check ids in trace_ids.

Cause 2 of 2 — the CloudFormation-provisioned Lambda execution-role trust check never completes.
AWS: a stack declaring an AWS::IAM::Role whose trust policy names lambda.amazonaws.com reaches CREATE_COMPLETE and the role is immediately assumable by Lambda, so the check settles well inside a minute.
Emulator: recorded behavior only — the check makes no progress and the harness reports "Check timed out after 60.0s", so no verdict about the trust policy is ever reached.
Why: a client polling the stack or the role never observes the terminal state it needs before wiring the function.
Fix: whichever of the CloudFormation stack-event/resource progression or the IAM role-assumption path the check polls; it must reach a terminal state.
Traces: apigateway__cloudformation__lambda.cfn-lambda-role-trust-policy-positive.

## Evidence
Cause 1: AssertionError: lambda invocation failed: {'errorMessage': "Unable to import module 'index': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', 'stackTrace': []}. Expected: the handler's own result, e.g. {"deployment_id": "<id>"} or {"error": "AccessDeniedException"}.
Cause 2: Error: Check timed out after 60.0s. Expected: a terminal CREATE_COMPLETE stack and an assumable role within the window.

## Steps to Reproduce
1. CreateFunction (python3.12) whose index.py does `import boto3` and calls apigateway.create_deployment / cloudformation.get_template, then Invoke it -> Runtime.ImportModuleError (cause 1).
2. CreateStack with an AWS::IAM::Role trusting lambda.amazonaws.com and poll DescribeStacks / assume it from Lambda -> never settles (cause 2).
