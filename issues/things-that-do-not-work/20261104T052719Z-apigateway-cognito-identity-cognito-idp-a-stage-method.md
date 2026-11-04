Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-identity__cognito-idp.cognito-identity-apigw-missing-execute-api-permission-negative
Issue-Key: apigateway__cognito-identity__cognito-idp.cognito-identity-apigw-missing-execute-api-permission-negative|run#12|AssertionError|assert status == 403
Issue-Commit: 024100b485faef6e8a7d69224b42c4d112173d44

# apigateway__cognito-identity__cognito-idp: a stage method protected by AWS_IAM serves credentials vended by a Cognito identity pool role that has no execute-api:Invoke permission

> _Note: this issue was identified against the code base at commit `024100b485faef6e8a7d69224b42c4d112173d44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: execute-api:Invoke is not authorized for Cognito-Identity-vended credentials.
AWS: credentials obtained from a Cognito identity pool (GetId → GetOpenIdToken / GetCredentialsForIdentity) assume the pool's authenticated/unauthenticated role, and a SigV4 request signed with them against a method whose `authorizationType=AWS_IAM` is served only if that role's identity policy allows `execute-api:Invoke` on the method ARN. With the role carrying no `execute-api` permission, the deployed stage refuses the signed request HTTP 403 with `x-amzn-ErrorType: AccessDeniedException` and the integration is never reached. recorded behavior only — the API Gateway Smithy model covers the control plane and does not describe the execute-api data plane.
Emulator: the signed request is not refused — the assertion `status == 403` fails, so the stage answered something other than 403 for credentials with no execute-api grant.
Why: an identity pool is the standard way to hand short-lived, deliberately under-privileged credentials to untrusted clients (mobile/browser). If the stage ignores the vended role's permissions, every unauthenticated or guest identity reaches every AWS_IAM-protected method — the identity pool's role scoping provides no boundary at all, and the application cannot test its own 403 handling.
Fix: the execute-api data-plane authorizer in the apigateway service crate must evaluate the identity policies of the role behind the presented session credentials (including sessions minted by cognito-identity) and refuse with 403 AccessDeniedException before integration dispatch. This is the same authorizer gap reported for the `apigateway__iam` heading, but that issue carries a different service token and a different replay set, so this check is tracked here.
Traces: apigateway__cognito-identity__cognito-idp.cognito-identity-apigw-missing-execute-api-permission-negative.

## Evidence
apigateway__cognito-identity__cognito-idp.cognito-identity-apigw-missing-execute-api-permission-negative:
```
File "check_apigateway__cognito_identity__cognito_idp_cognito_identity_apigw_missing_execute_api_permission_negative.py", line 58, in run
assert status == 403
AssertionError
```
The check signs the stage request with botocore SigV4 using the Cognito-Identity-vended Credentials for an identity-pool role carrying no execute-api permission. Expected: HTTP 403 with x-amzn-ErrorType AccessDeniedException. Observed: a non-403 response (the request was not refused).

## Steps to Reproduce
1. Build and boot the emulator from the checkout and replay the behavioral suite.
2. Re-replay apigateway__cognito-identity__cognito-idp.cognito-identity-apigw-missing-execute-api-permission-negative.
3. Manual repro: create a Cognito user pool and identity pool; create an IAM role with a cognito-identity trust policy and NO execute-api:Invoke statement and wire it as the pool's authenticated role; CreateRestApi with a resource whose method has authorizationType=AWS_IAM and a MOCK integration; CreateDeployment. Obtain credentials via cognito-identity GetId + GetCredentialsForIdentity, SigV4-sign a GET to the stage URL with them, and send it. Observed: not a 403. Expected: 403 with header x-amzn-ErrorType: AccessDeniedException and no integration body.
