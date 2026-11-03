Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-idp__lambda.cli-apigw-lambda-cognito-full-stack-smoke apigateway__cognito-idp__lambda.cli-cognito-admin-create-user-lambda-invoke apigateway__cognito-idp__lambda.cli-lambda-cognito-trigger-setup-and-test
Issue-Key: apigateway__cognito-idp__lambda.cli-apigw-lambda-cognito-full-stack-smoke|_http_get#39|AssertionError|raise AssertionError(f"GET {url} never resolved within {timeout_s}s: {last_exc}")
Issue-Key: apigateway__cognito-idp__lambda.cli-cognito-admin-create-user-lambda-invoke|_run_shell#29|AssertionError|raise AssertionError(
Issue-Key: apigateway__cognito-idp__lambda.cli-lambda-cognito-trigger-setup-and-test|run#28|AssertionError|assert signup_out["UserConfirmed"] is True, (
Issue-Commit: dc7d1cb059ddc05800728693f0165ec8a2a1b13e

# apigateway__cognito-idp__lambda: 3 net-new causes — a deployed execute-api stage answers 500 on a Cognito-authorized method, Lambda Invoke answers NotImplemented from the CLI, and the PreSignUp trigger's autoConfirmUser is ignored

> _Note: this issue was identified against the code base at commit `dc7d1cb059ddc05800728693f0165ec8a2a1b13e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
Three distinct root causes, all grounded in failing behavioral checks under the suite heading `apigateway__cognito-idp__lambda`. A fixer must close all three before submitting.

Cause 1 of 3: a GET against a deployed REST API stage whose method is guarded by a COGNITO_USER_POOLS authorizer answers HTTP 500 instead of invoking the integration.
AWS: an API Gateway stage URL (`https://{api-id}.execute-api.{region}.amazonaws.com/{stage}/{path}`) with a COGNITO_USER_POOLS authorizer and a valid `Authorization: <IdToken>` header validates the token and invokes the AWS_PROXY integration, returning the handler's own status/body (200 here). Recorded behavior only for the stage path shape the emulator exposes. Smithy: apigateway has no modelled error for the data plane; the stage URL is the service's data plane, not a modelled operation.
Emulator: the GET answers `HTTP Error 500: Internal Server Error` for every attempt until the check's 45 s resolve budget expires. Replay detail: `AssertionError: GET .../_aws/execute-api/<api-id>/prod/whoami never resolved within 45.0s: HTTP Error 500: Internal Server Error`.
Why: a client that deploys a Cognito-protected REST API and calls its stage URL gets an opaque 500 instead of the integration's response, so no authorizer/integration behaviour can be exercised end-to-end and 500 is indistinguishable from a real backend fault.
Fix: the apigateway data-plane stage router/invoke path (the execute-api request handler), not the authorizer model. Related open issue `20261029T182116Z-apigateway-cognito-idp-lambda-execute-api-invocations-answer.md` describes execute-api invocations for this service pair but does not carry this check's replay key.
Traces: apigateway__cognito-idp__lambda.cli-apigw-lambda-cognito-full-stack-smoke

Cause 2 of 3: lambda Invoke refuses a real invocation with NotImplemented, so a CLI-driven Cognito+Lambda flow cannot run.
AWS: Smithy: lambda `Invoke` returns 200 with `Payload` carrying the handler's result; its modelled errors are InvalidParameterValueException, ResourceNotFoundException, RequestTooLargeException, UnsupportedMediaTypeException, TooManyRequestsException, ServiceException, and friends — `NotImplemented` is not among them. A python3.12 deployment package must actually run.
Emulator: `An error occurred (NotImplemented) when calling the Invoke operation: Invoke resolved arn:aws:lambda:<region>:<account>:function:<name> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator...` (aws CLI exits rc=254).
Why: every cross-service scenario that depends on the function producing a side effect or a payload fails at the first invoke; a client sees an unmodelled error code it cannot handle.
Fix: the lambda Invoke handler / its in-process runtime. Related open issue `20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md` describes the same refusal but does not carry this check's replay key.
Traces: apigateway__cognito-idp__lambda.cli-cognito-admin-create-user-lambda-invoke

Cause 3 of 3: a PreSignUp Lambda trigger that sets `autoConfirmUser` is ignored, so SignUp reports UserConfirmed=false.
AWS: a user pool with a PreSignUp Lambda trigger has the trigger invoked synchronously during SignUp; when the handler returns `response.autoConfirmUser = true`, Cognito confirms the user and SignUp's response carries `UserConfirmed: true`. Recorded behavior only for the flag; the trigger contract is documented by Cognito's PreSignUp trigger reference.
Emulator: SignUp returns `{'UserConfirmed': False, 'UserSub': '<uuid>'}` — the trigger's response is not applied (and, per cause 2, the trigger is very likely never invoked at all).
Why: a client that relies on auto-confirmation must then run a ConfirmSignUp it never codes for, and any sign-in immediately after SignUp fails with UserNotConfirmedException.
Fix: cognito-idp SignUp's trigger dispatch and the application of the PreSignUp response (autoConfirmUser / autoVerifyEmail / autoVerifyPhone) to the created user's state. Related open issue `20261025T022656Z-cognito-idp-lambda-presignup-auto-confirm-is-ignored.md` is filed under a different service heading and does not carry this check's replay key.
Traces: apigateway__cognito-idp__lambda.cli-lambda-cognito-trigger-setup-and-test

## Evidence
Cause 1 — expected: 200 from the AWS_PROXY integration for `GET /{stage}/whoami` with a valid Cognito IdToken. Actual: `HTTP Error 500: Internal Server Error` on every poll for 45 s.
Cause 2 — expected: Invoke 200 with the handler's Payload. Actual: HTTP 501-class `NotImplemented` with message `Invoke resolved <function-arn> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator: there is no in-process runtime for it, and no response payload is returned that the function did not produce. InvocationType=DryRun validates an invocation without running the function.` (fixed text; the ARN and runtime are values from the request/state).
Cause 3 — expected: `SignUp` -> `{"UserConfirmed": true, "UserSub": "<uuid>"}` after a PreSignUp handler sets autoConfirmUser. Actual: `{'UserConfirmed': False, 'UserSub': '<uuid>'}`.

## Steps to Reproduce
1. Replay the behavioral suite and re-run the three checks listed in trace_ids.
2. Cause 1: create a REST API with a COGNITO_USER_POOLS authorizer over a user pool, an AWS_PROXY integration to a Lambda, deploy to a stage, sign a user in, then GET the stage URL with `Authorization: <IdToken>`; observe 500 instead of the handler's 200.
3. Cause 2: create any python3.12 function and run `aws lambda invoke --function-name <fn> out.json`; observe the NotImplemented error.
4. Cause 3: create a user pool with a PreSignUp trigger whose handler sets `response['autoConfirmUser'] = True`, call SignUp, and read `UserConfirmed` in the response.
