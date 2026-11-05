Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-idp__lambda.lambda-apigateway-create-deployment-missing-permission-negative apigateway__cognito-idp__lambda.lambda-apigateway-create-deployment-positive apigateway__cognito-idp__lambda.lambda-apigateway-get-resources-positive apigateway__cognito-idp__lambda.lambda-apigateway-get-rest-apis-missing-permission-negative apigateway__cognito-idp__lambda.lambda-apigateway-get-rest-apis-positive apigateway__cognito-idp__lambda.lambda-apigateway-update-stage-variables-positive apigateway__cognito-idp__lambda.lambda-cognito-admin-create-user-missing-permission-negative apigateway__cognito-idp__lambda.lambda-cognito-admin-create-user-positive apigateway__cognito-idp__lambda.lambda-cognito-admin-disable-user-missing-permission-negative apigateway__cognito-idp__lambda.lambda-cognito-admin-get-user-positive apigateway__cognito-idp__lambda.lambda-cognito-admin-set-user-password-positive apigateway__cognito-idp__lambda.lambda-cognito-list-users-positive apigateway__cognito-idp__lambda.lambda-cognito-wrong-user-pool-id-negative apigateway__cognito-idp__lambda.cli-cognito-admin-create-user-lambda-invoke apigateway__cognito-idp__lambda.cognito-post-confirmation-trigger-invokes-lambda apigateway__cognito-idp__lambda.cognito-trigger-missing-invoke-permission-negative
Issue-Key: apigateway__cognito-idp__lambda.lambda-apigateway-create-deployment-missing-permission-negative|_invoke#8|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: apigateway__cognito-idp__lambda.lambda-apigateway-create-deployment-positive|_invoke#8|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: apigateway__cognito-idp__lambda.lambda-apigateway-get-resources-positive|_invoke#9|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: apigateway__cognito-idp__lambda.lambda-apigateway-get-rest-apis-missing-permission-negative|_invoke#5|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: apigateway__cognito-idp__lambda.lambda-apigateway-get-rest-apis-positive|_invoke#6|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: apigateway__cognito-idp__lambda.lambda-apigateway-update-stage-variables-positive|_invoke#10|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: apigateway__cognito-idp__lambda.lambda-cognito-admin-create-user-missing-permission-negative|_invoke_lambda#6|AssertionError|raise AssertionError(f"lambda invocation raised FunctionError: {body}")
Issue-Key: apigateway__cognito-idp__lambda.lambda-cognito-admin-create-user-positive|_invoke_lambda#5|AssertionError|raise AssertionError(f"lambda invocation raised FunctionError: {body}")
Issue-Key: apigateway__cognito-idp__lambda.lambda-cognito-admin-disable-user-missing-permission-negative|_invoke_lambda#15|AssertionError|raise AssertionError(f"lambda invocation raised FunctionError: {body}")
Issue-Key: apigateway__cognito-idp__lambda.lambda-cognito-admin-get-user-positive|_invoke_lambda#8|AssertionError|raise AssertionError(f"lambda invocation raised FunctionError: {body}")
Issue-Key: apigateway__cognito-idp__lambda.lambda-cognito-admin-set-user-password-positive|_invoke_lambda#11|AssertionError|raise AssertionError(f"lambda invocation raised FunctionError: {body}")
Issue-Key: apigateway__cognito-idp__lambda.lambda-cognito-list-users-positive|_invoke_lambda#7|AssertionError|raise AssertionError(f"lambda invocation raised FunctionError: {body}")
Issue-Key: apigateway__cognito-idp__lambda.lambda-cognito-wrong-user-pool-id-negative|_invoke_lambda#7|AssertionError|raise AssertionError(f"lambda invocation raised FunctionError: {body}")
Issue-Key: apigateway__cognito-idp__lambda.cli-cognito-admin-create-user-lambda-invoke|run#30|KeyError|assert invoke_result["username"] == username, invoke_result
Issue-Key: cognito-identity-provider|AdminConfirmSignUp|UserLambdaValidationException
Issue-Key: apigateway__cognito-idp__lambda.cognito-trigger-missing-invoke-permission-negative|run#11|AssertionError|assert code_before == "UnexpectedLambdaException", (
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# apigateway__cognito-idp__lambda: the Lambda python runtime has no AWS SDK, so every in-function API Gateway/Cognito call and every Cognito trigger fails at init

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 2 — the managed python runtime ships no boto3/botocore.
AWS: the python3.x managed runtimes bundle boto3/botocore, so a handler can `import boto3` and call apigateway:* and cognito-idp:Admin* under its execution role, returning its own result (or the service's error code when the role lacks the action).
Emulator: HTTP 200, FunctionError=Unhandled, payload {"errorMessage":"Unable to import module '<module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError"}. Downstream, checks that read a field of the handler's result instead raise KeyError (e.g. KeyError: 'username') because the error payload has no such field.
Why: none of the positive paths nor the missing-permission controls can be observed; a client sees an init failure rather than the API result or the AccessDeniedException it expects.
Fix: the python runtime's bundled site-packages / module search path in the lambda runtime component.
Traces: the 14 lambda-apigateway-*/lambda-cognito-*/cli-cognito-* ids in trace_ids.

Cause 2 of 2 — a Cognito user-pool Lambda trigger surfaces the runtime's init failure as a user-pool validation error, and the trigger-permission control never settles.
AWS: PostConfirmation and the other user-pool triggers invoke the configured function; when the function faults, Cognito raises UserLambdaValidationException with the trigger name and the function's error — but a correctly packaged handler that imports boto3 does not fault. When the function's resource policy does not grant cognito-idp.amazonaws.com, the pool operation is refused with InvalidLambdaResponseException/AccessDenied naming the missing permission.
Emulator: AdminConfirmSignUp fails with UserLambdaValidationException "PostConfirmation failed with error Unable to import module 'index': No module named 'boto3'." — i.e. the trigger does run, but the runtime defect is what it reports; and the missing-invoke-permission negative never reaches its assertion.
Why: the trigger chain cannot be exercised, and the refusal a client would match on is replaced by an unrelated import error.
Fix: once the runtime provides boto3, the Cognito trigger invocation path and its permission check must still be re-replayed; the error mapping lives in the cognito-idp user-pool trigger handler.
Traces: apigateway__cognito-idp__lambda.cognito-post-confirmation-trigger-invokes-lambda, apigateway__cognito-idp__lambda.cognito-trigger-missing-invoke-permission-negative.

## Evidence
Cause 1: AssertionError: lambda invocation errored / raised FunctionError: {'errorMessage': "Unable to import module 'index': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError'}; and KeyError: 'username' where the check read invoke_result['username'].
Cause 2: botocore.errorfactory.UserLambdaValidationException: An error occurred (UserLambdaValidationException) when calling the AdminConfirmSignUp operation: PostConfirmation failed with error Unable to import module 'index': No module named 'boto3'. Expected: AdminConfirmSignUp succeeds and the trigger's side effect is observable.

## Steps to Reproduce
1. CreateFunction (python3.12) importing boto3 and calling apigateway.get_rest_apis / cognito_idp.admin_create_user; Invoke -> Runtime.ImportModuleError (cause 1).
2. CreateUserPool with a PostConfirmation LambdaConfig pointing at such a function, SignUp then AdminConfirmSignUp -> UserLambdaValidationException; repeat without lambda:InvokeFunction for cognito-idp.amazonaws.com (cause 2).
