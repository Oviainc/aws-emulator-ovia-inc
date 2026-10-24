Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-idp__lambda.cfn-cognito-userpool-lambda-trigger-config-positive apigateway__cognito-idp__lambda.cognito-custom-message-trigger-modifies-message apigateway__cognito-idp__lambda.cognito-post-confirmation-trigger-invokes-lambda apigateway__cognito-idp__lambda.cognito-trigger-missing-invoke-permission-negative apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-claims-context-passthrough apigateway__cognito-identity__cognito-idp.cognito-identity-getid-with-userpool-token-positive apigateway__cognito-identity__cognito-idp.cognito-identity-wrong-login-provider-negative apigateway__cognito-idp__lambda.cognito-define-auth-challenge-trigger-chain apigateway__cognito-idp__lambda.apigw-cognito-authorizer-tampered-signature-negative
Issue-Key: cognito-identity-provider|SignUp|UnknownService
Issue-Key: apigateway__cognito-identity__cognito-idp|GetUser|UnknownService
Issue-Key: cognito-identity|GetId|UnknownService
Issue-Key: apigateway__cognito-identity__cognito-idp.cognito-identity-wrong-login-provider-negative|run#8|AssertionError|assert code == "NotAuthorizedException", (
Issue-Key: cognito-identity-provider|AdminInitiateAuth|NotImplemented
Issue-Key: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-tampered-signature-negative|_tamper_signature#8|IndexError|flipped = ("A" if signature[0] != "A" else "B") + signature[1:]
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# Cognito: every unsigned (anonymous) operation answers UnknownService, AdminInitiateAuth CUSTOM_AUTH is unimplemented, and issued JWTs are alg=none with an empty signature (3 causes)

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SignUp

## Description
Three root causes in the Cognito surface, all grounded in failing checks the replay report marked neither ALREADY FILED nor OWNED.

Cause 1 of 3 — operations that are called WITHOUT SigV4 are not routed: the request router resolves the JSON-1.1 target prefix 'AWSCognitoIdentityProviderService' / 'AWSCognitoIdentityService' to a service key it has nothing registered under, and answers UnknownService.
AWS: Smithy marks the self-service Cognito operations anonymous (SignUp, ConfirmSignUp, InitiateAuth, and GetUser with an access token; cognito-identity GetId/GetCredentialsForIdentity) — they carry no SigV4 credential scope and are dispatched by the X-Amz-Target prefix. Real AWS serves them and returns the modeled result, or a modeled failure such as NotAuthorizedException for a login provider the identity pool does not trust.
Emulator: HTTP error (UnknownService) 'no emulated service is registered for "awscognitoidentityproviderservice"' (and '...awscognitoidentityservice' for cognito-identity), while SigV4-signed operations on the SAME service (AdminCreateUser, AdminInitiateAuth) are routed normally. The router's target-prefix lookup does not map to the registered service, so only the credential-scope path works.
Why: no user can sign up, confirm, sign in or read their own profile; every user-pool Lambda trigger scenario and every identity-pool credential exchange is unreachable, and a negative case that must fail with NotAuthorizedException fails with UnknownService instead — the wrong error class entirely.
Fix: the request router's service resolution — the X-Amz-Target prefix (and Host) must resolve to the same registered service the SigV4 signing name resolves to. Open issue 20261022T220446Z-cognito-idp-initiateauth-is-refused-with-unknownservice-no-emulated-service.md records the same defect for InitiateAuth only; it does not carry the replay keys below (SignUp, GetUser, GetId), so a fixer could close it without re-running them.
Traces: apigateway__cognito-idp__lambda.cfn-cognito-userpool-lambda-trigger-config-positive, apigateway__cognito-idp__lambda.cognito-custom-message-trigger-modifies-message, apigateway__cognito-idp__lambda.cognito-post-confirmation-trigger-invokes-lambda, apigateway__cognito-idp__lambda.cognito-trigger-missing-invoke-permission-negative, apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-claims-context-passthrough, apigateway__cognito-identity__cognito-idp.cognito-identity-getid-with-userpool-token-positive, apigateway__cognito-identity__cognito-idp.cognito-identity-wrong-login-provider-negative.

Cause 2 of 3 — AdminInitiateAuth refuses AuthFlow=CUSTOM_AUTH with NotImplemented.
AWS: Smithy AdminInitiateAuth accepts AuthFlowType CUSTOM_AUTH and answers with a ChallengeName/Session pair driven by the DefineAuthChallenge trigger; the operation itself is implemented in this emulator for other flows.
Emulator: (NotImplemented) 'operation AdminInitiateAuth with AuthFlow=CUSTOM_AUTH is scaffolded but not implemented in this emulator'. No per-operation stub issue covers this, because the operation is not a stub — only this flow is missing.
Why: the whole custom authentication challenge chain (DefineAuthChallenge / CreateAuthChallenge / VerifyAuthChallengeResponse) cannot be exercised.
Fix: the CUSTOM_AUTH branch of AdminInitiateAuth's handler (and the challenge state it must create).
Traces: apigateway__cognito-idp__lambda.cognito-define-auth-challenge-trigger-chain.

Cause 3 of 3 — the tokens Cognito issues are unsigned: header alg='none' and an empty third JWT segment.
AWS: recorded behavior only — Cognito issues RS256-signed JWTs whose third segment is a non-empty base64url signature verifiable against the pool's JWKS; this is what makes a tampered-signature test meaningful.
Emulator: AuthenticationResult tokens decode to header {"alg":"none","typ":"JWT"} and end in a trailing '.' with nothing after it, so a consumer that flips the last signature character has no character to flip (the check aborts with IndexError: string index out of range).
Why: no client or authorizer can verify a token, and no negative test can distinguish a valid token from a tampered one — signature verification is unreproducible.
Fix: token minting in the user-pool auth operations (sign with a per-pool key and publish the matching JWKS).
Traces: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-tampered-signature-negative.

## Evidence
Cause 1: botocore.exceptions.ClientError: An error occurred (UnknownService) when calling the SignUp operation: no emulated service is registered for 'awscognitoidentityproviderservice'; same for GetUser, and 'no emulated service is registered for "awscognitoidentityservice"' when calling GetId. Negative case: AssertionError: expected NotAuthorizedException for a real, validly-signed token from a user pool NOT registered as a login provider on this identity pool, got 'UnknownService'. Contrast: AdminCreateUser on the same service is routed and answers a modeled InvalidParameterException.
Cause 2: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AdminInitiateAuth operation: operation AdminInitiateAuth with AuthFlow=CUSTOM_AUTH is scaffolded but not implemented in this emulator.
Cause 3: IndexError: string index out of range at the tamper step; the returned AccessToken/IdToken/RefreshToken all decode to header eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0 ({"alg":"none","typ":"JWT"}) and end with '.' and an empty signature segment.

## Steps to Reproduce
1. CreateUserPool + CreateUserPoolClient, then call SignUp with no credentials (the operation is anonymous) — expect 200 with UserConfirmed/UserSub, observe UnknownService. Same for GetUser with an access token and for cognito-identity GetId.
2. CreateUserPool with a DefineAuthChallenge trigger, then AdminInitiateAuth(AuthFlow='CUSTOM_AUTH') — expect a ChallengeName/Session, observe NotImplemented.
3. AdminInitiateAuth(ADMIN_USER_PASSWORD_AUTH) and base64-decode the returned IdToken header — expect alg RS256 and a non-empty signature segment, observe alg 'none' and an empty signature.
