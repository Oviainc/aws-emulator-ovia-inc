Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-identity__cognito-idp.cognito-identity-get-credentials-for-identity-positive
Issue-Key: apigateway__cognito-identity__cognito-idp.cognito-identity-get-credentials-for-identity-positive|run#14|IndexError|assumed_role_name = caller["Arn"].split(":assumed-role/", 1)[1].split("/", 1)[0]
Issue-Commit: d0dc720ebbf11903c1fa9407d1c82e53eafd4b75

# apigateway__cognito-identity__cognito-idp: the credentials GetCredentialsForIdentity vends carry no assumed-role caller identity, so the API Gateway federation scenario cannot identify the session (1 cause)

> _Note: this issue was identified against the code base at commit `d0dc720ebbf11903c1fa9407d1c82e53eafd4b75`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
ONE root cause (cause 1 of 1).

AWS: GetCredentialsForIdentity returns the credentials of an sts:AssumeRoleWithWebIdentity session for the role the identity pool resolves, so sts:GetCallerIdentity under those credentials answers an ARN of the form arn:aws:sts::<account>:assumed-role/<role-name>/CognitoIdentityCredentials — the form every federated-caller scenario parses to learn which role was vended (Smithy GetCredentialsForIdentity: output Credentials {AccessKeyId, SecretKey, SessionToken, Expiration}; auth [] / optionalAuth true).
Emulator: the vended credentials resolve to the emulator's ambient principal, arn:aws:iam::000000000000:user/emulator — an IAM user ARN with no ':assumed-role/' segment at all, so splitting the ARN on ':assumed-role/' yields nothing and the check dies with IndexError before it can assert anything about the role.
Why: any client (or API Gateway integration) that identifies the federated session from its caller ARN gets a principal that is not a role session and is shared by every caller, so the identity of a Cognito-federated request cannot be established and no per-role authorization can be applied to it.
Fix: the cognito-identity GetCredentialsForIdentity handler (services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/get_credentials_for_identity.rs) must mint the session through the emulator's STS AssumeRoleWithWebIdentity path for the resolved role, so the credentials map to an assumed-role principal.
Related: the companion issue filed in this pass under service token cognito-identity records the same defect from the cognito-identity-only checks; this check is in a different service group and is not carried by it.
Traces: apigateway__cognito-identity__cognito-idp.cognito-identity-get-credentials-for-identity-positive.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1:

apigateway__cognito-identity__cognito-idp.cognito-identity-get-credentials-for-identity-positive
  File "check_apigateway__cognito_identity__cognito_idp_cognito_identity_get_credentials_for_identity_positive.py", line 62, in run
    assumed_role_name = caller["Arn"].split(":assumed-role/", 1)[1].split("/", 1)[0]
  IndexError: list index out of range

The caller ARN produced under the vended credentials elsewhere in the same run is 'arn:aws:iam::000000000000:user/emulator' (see the cognito-identity checks in this suite), which contains no ':assumed-role/' segment; AWS returns 'arn:aws:sts::<account>:assumed-role/<role>/CognitoIdentityCredentials'.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; re-replay the check id in trace_ids.
2. Manual repro: CreateIdentityPool(AllowUnauthenticatedIdentities=True); create an IAM role trusting cognito-identity.amazonaws.com for sts:AssumeRoleWithWebIdentity; SetIdentityPoolRoles(Roles={'unauthenticated': <role arn>}); GetId; GetCredentialsForIdentity; build an sts client from the returned Credentials and call GetCallerIdentity -> expect arn:aws:sts::<account>:assumed-role/<role>/CognitoIdentityCredentials; observe arn:aws:iam::000000000000:user/emulator.
