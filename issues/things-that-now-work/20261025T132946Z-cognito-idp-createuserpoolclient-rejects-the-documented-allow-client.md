Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.id-token-and-m2m-token-are-refused-where-a-user-access-token-is-required
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-identity-provider|CreateUserPoolClient|InvalidParameterException
Issue-Commit: eaacfa963fb5bc737a9bad8d07574a87d1e0bb31

# cognito-idp: CreateUserPoolClient rejects the documented ALLOW_CLIENT_TOKEN_AUTH explicit auth flow, making M2M (GetClientToken) app clients impossible to create (1 cause)

> _Note: this issue was identified against the code base at commit `eaacfa963fb5bc737a9bad8d07574a87d1e0bb31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUserPoolClient

## Description
Cause 1 of 1: the enum value set the emulator validates CreateUserPoolClient.ExplicitAuthFlows against omits ALLOW_CLIENT_TOKEN_AUTH, so the only way to configure a machine-to-machine app client is refused outright. Failing check: cognito-idp.id-token-and-m2m-token-are-refused-where-a-user-access-token-is-required.

AWS: a machine-to-machine app client is created by CreateUserPoolClient with GenerateSecret=true and ExplicitAuthFlows containing ALLOW_CLIENT_TOKEN_AUTH, and that call succeeds. Smithy (cognito-idp, services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/smithy-model): operation GetClientToken is modeled (input ClientId+Secret required, Scopes; output ClientAuthenticationResult) and its own documentation states the requirement verbatim — "To use this operation, you must configure the app client with a client secret and enable the ALLOW_CLIENT_TOKEN_AUTH authentication flow. The ALLOW_CLIENT_TOKEN_AUTH flow is mutually exclusive with user authentication flows. It must be the only authentication flow that you configure for the app client." CreateUserPoolClient.ExplicitAuthFlows targets ExplicitAuthFlowsListType -> ExplicitAuthFlowsType, whose checked-in value list is [ADMIN_NO_SRP_AUTH, CUSTOM_AUTH_FLOW_ONLY, USER_PASSWORD_AUTH, ALLOW_ADMIN_USER_PASSWORD_AUTH, ALLOW_CUSTOM_AUTH, ALLOW_USER_PASSWORD_AUTH, ALLOW_USER_SRP_AUTH, ALLOW_REFRESH_TOKEN_AUTH, ALLOW_USER_AUTH] — it does NOT contain ALLOW_CLIENT_TOKEN_AUTH even though the same model's GetClientToken documentation requires that value. So the model is internally inconsistent and the enum is the stale half: the value is a documented, required input of a modeled operation.

Emulator: the call is refused with HTTP 400 InvalidParameterException and the message "1 validation error detected: Value 'ALLOW_CLIENT_TOKEN_AUTH' at 'explicitAuthFlows.1.member' failed to satisfy constraint: Member must satisfy enum value set: [ADMIN_NO_SRP_AUTH, CUSTOM_AUTH_FLOW_ONLY, USER_PASSWORD_AUTH, ALLOW_ADMIN_USER_PASSWORD_AUTH, ALLOW_CUSTOM_AUTH, ALLOW_USER_PASSWORD_AUTH, ALLOW_USER_SRP_AUTH, ALLOW_REFRESH_TOKEN_AUTH, ALLOW_USER_AUTH]". In that message the fixed text is the AWS enum-constraint template ("<n> validation error(s) detected: Value '<value>' at '<member path>' failed to satisfy constraint: Member must satisfy enum value set: [<values>]"); the variable parts are the rejected value, the 1-based member path taken from the request's list position, and the accepted value list taken from the enum — none of those recorded strings is itself the requirement, the requirement is that a request naming ALLOW_CLIENT_TOKEN_AUTH is NOT rejected at all.

Why: a client cannot create an M2M app client, so the entire client-credentials surface of the emulator is unreachable — GetClientToken can never be called with a correctly configured client, and no caller can obtain a client_credentials access token to exercise resource-server scopes. Any application or test that provisions an M2M client as its first step fails at provisioning with a validation error that (wrongly) tells the developer the documented value does not exist. A second consequence: because provisioning dies here, the authorization asymmetry the check exists to grade is never exercised at all — the emulator's handling of an ID token and of a scope-less M2M token at GetUser/UpdateUserAttributes (AWS refuses both with NotAuthorizedException, message "Invalid Access Token" for the ID token and "Access Token does not have required scopes" for the client_credentials token, with no profile mutation) remains completely unverified, so fixing only the enum may expose a further divergence on the same check. Note the check's own header marks it as never replayed against real AWS, so those two message strings are its stated hypotheses; the ALLOW_CLIENT_TOKEN_AUTH requirement is not a hypothesis — it is stated by the service model's own GetClientToken documentation.

Fix: the ExplicitAuthFlows enum/validation used by the CreateUserPoolClient handler (and the matching UpdateUserPoolClient path and the UserPoolClientType serializer that echoes ExplicitAuthFlows back) in the cognito-idp service crate: ALLOW_CLIENT_TOKEN_AUTH must be an accepted member value, must be accepted together with GenerateSecret=true / a client secret, and must round-trip in the DescribeUserPoolClient/CreateUserPoolClient response. The service's stale ExplicitAuthFlowsType value list is where the gap lives. Per the model documentation the flow is also mutually exclusive with the user auth flows, so a client combining it with ALLOW_USER_PASSWORD_AUTH/ALLOW_USER_SRP_AUTH/ALLOW_USER_AUTH/ALLOW_CUSTOM_AUTH/ALLOW_ADMIN_USER_PASSWORD_AUTH must still be rejected — Hypothesis: that rejection is InvalidParameterException, since the model does not state the code for the mutual-exclusivity violation and no recording of it exists here. Unrelated open issue 20261024T114943Z-cognito-idp-user-pool-and-app-client-configuration.md covers other app-client configuration validation gaps but names neither ALLOW_CLIENT_TOKEN_AUTH nor GetClientToken and does not carry this check's replay key.

Traces: cognito-idp.id-token-and-m2m-token-are-refused-where-a-user-access-token-is-required (fails at the CreateUserPoolClient call that provisions the M2M client).

## Evidence
Check cognito-idp.id-token-and-m2m-token-are-refused-where-a-user-access-token-is-required, verdict [error]:

  botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameterException) when calling the CreateUserPoolClient operation: 1 validation error detected: Value 'ALLOW_CLIENT_TOKEN_AUTH' at 'explicitAuthFlows.1.member' failed to satisfy constraint: Member must satisfy enum value set: [ADMIN_NO_SRP_AUTH, CUSTOM_AUTH_FLOW_ONLY, USER_PASSWORD_AUTH, ALLOW_ADMIN_USER_PASSWORD_AUTH, ALLOW_CUSTOM_AUTH, ALLOW_USER_PASSWORD_AUTH, ALLOW_USER_SRP_AUTH, ALLOW_REFRESH_TOKEN_AUTH, ALLOW_USER_AUTH]

The refused request (from the check body) is:
  CreateUserPoolClient(UserPoolId=<pool>, ClientName=<unique>, GenerateSecret=True,
                       ExplicitAuthFlows=['ALLOW_CLIENT_TOKEN_AUTH'],
                       AllowedOAuthFlows=['client_credentials'],
                       AllowedOAuthFlowsUserPoolClient=True,
                       AllowedOAuthScopes=['<resource-server-id>/read'])

Expected (AWS, per the service model's own GetClientToken documentation): 200 with UserPoolClient echoing ExplicitAuthFlows=['ALLOW_CLIENT_TOKEN_AUTH'] and a ClientSecret, after which GetClientToken(ClientId, Secret, Scopes=['<rs>/read']) returns ClientAuthenticationResult.AccessToken.
Actual (emulator): HTTP 400 InvalidParameterException, no client created, and GetClientToken therefore unreachable.

Corroborating model evidence read from the checkout:
- smithy_query shape ExplicitAuthFlowsType -> values [ADMIN_NO_SRP_AUTH, CUSTOM_AUTH_FLOW_ONLY, USER_PASSWORD_AUTH, ALLOW_ADMIN_USER_PASSWORD_AUTH, ALLOW_CUSTOM_AUTH, ALLOW_USER_PASSWORD_AUTH, ALLOW_USER_SRP_AUTH, ALLOW_REFRESH_TOKEN_AUTH, ALLOW_USER_AUTH] (no ALLOW_CLIENT_TOKEN_AUTH) — exactly the list in the emulator's error message.
- smithy_query doc GetClientToken -> "you must configure the app client with a client secret and enable the ALLOW_CLIENT_TOKEN_AUTH authentication flow ... It must be the only authentication flow that you configure for the app client."

This check was NOT reported as blocked by an unimplemented scaffold stub and is NOT owned by any open issue in this report.

## Steps to Reproduce
1. cognito-idp:CreateUserPool (any name) -> note UserPoolId.
2. cognito-idp:CreateResourceServer(UserPoolId=<pool>, Identifier='<rs-id>', Name='rs', Scopes=[{ScopeName:'read', ScopeDescription:'read access'}]).
3. cognito-idp:CreateUserPoolClient(UserPoolId=<pool>, ClientName='m2m', GenerateSecret=True, ExplicitAuthFlows=['ALLOW_CLIENT_TOKEN_AUTH'], AllowedOAuthFlows=['client_credentials'], AllowedOAuthFlowsUserPoolClient=True, AllowedOAuthScopes=['<rs-id>/read']).
   AWS: 200, the client is created with that flow and a ClientSecret.
   Emulator: HTTP 400 InvalidParameterException "1 validation error detected: Value 'ALLOW_CLIENT_TOKEN_AUTH' at 'explicitAuthFlows.1.member' failed to satisfy constraint: Member must satisfy enum value set: [...]".
4. After the fix, confirm the follow-on path the refusal currently hides: cognito-idp:GetClientToken(ClientId=<m2m client>, Secret=<its secret>, Scopes=['<rs-id>/read']) returns ClientAuthenticationResult.AccessToken whose scope claim is exactly '<rs-id>/read', which carries no 'username' claim and not the aws.cognito.signin.user.admin scope; then GetUser and UpdateUserAttributes with that token, and with the ID token from a USER_PASSWORD_AUTH sign-in, must both be refused NotAuthorizedException without mutating the profile.
5. Also confirm the mutual-exclusivity rule is still enforced: CreateUserPoolClient mixing ALLOW_CLIENT_TOKEN_AUTH with any user auth flow (e.g. ALLOW_USER_PASSWORD_AUTH) must be rejected.
6. Re-replay cognito-idp.id-token-and-m2m-token-are-refused-where-a-user-access-token-is-required.
