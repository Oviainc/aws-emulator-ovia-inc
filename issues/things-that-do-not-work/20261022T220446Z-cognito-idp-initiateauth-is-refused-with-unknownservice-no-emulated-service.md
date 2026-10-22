Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__cognito-idp.cognito-secret-request-parameters-are-hidden-in-the-delivered-record
Issue-Key: cognito-identity-provider|InitiateAuth|UnknownService
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# InitiateAuth is refused with UnknownService ('no emulated service is registered for awscognitoidentityproviderservice') although other cognito-idp operations are served

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth

## Description
Cause 1 of 1: the unsigned/anonymous Cognito Identity Provider operation does not resolve to the mounted cognito-idp service — the router looks it up under the service crate's internal name instead of the service's signing name, and answers `UnknownService`.

AWS: Smithy: `AWSCognitoIdentityProviderService` speaks awsJson1_1; `InitiateAuth` is one of the operations with an auth override (it is callable with NO SigV4 credentials — it is how a client obtains tokens), returns HTTP 200 with an InitiateAuthResponse (AuthenticationResult / ChallengeName / ChallengeParameters / Session) and models client errors such as NotAuthorizedException, UserNotFoundException, ResourceNotFoundException, InvalidParameterException. `UnknownService` is not a modeled error of this service, and the service is bootstrapped in this checkout (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice).
Emulator: `InitiateAuth` fails with code `UnknownService` and message `no emulated service is registered for 'awscognitoidentityproviderservice'` — the lookup key is the crate/service shape name, not the sigv4 signing name `cognito-idp`. Other operations of the SAME service in the same run DO reach it (the sibling check's `AdminGetUser` call succeeds), which is the discriminator: the failing call is the unsigned one, so there is no SigV4 credential scope to take the signing name from and the fallback key is wrong.
Why: no client can authenticate against the emulated user pool at all — InitiateAuth is the entry point of every Cognito sign-in flow (USER_PASSWORD_AUTH, SRP, refresh-token), so every downstream call that needs an access token is unreachable. The error code is also unmodeled, so SDK error handling cannot branch on it.
Fix: the route-key resolution / service-registry lookup for requests with no SigV4 Authorization header (`resolve_route_key` and the cognito-idp crate's `can_handle` in src/service.rs) — resolve an anonymous awsJson request to the service by its X-Amz-Target prefix or endpoint rather than by a name derived from the crate.
Traces: cloudtrail__cognito-idp.cognito-secret-request-parameters-are-hidden-in-the-delivered-record.

## Evidence
`botocore.exceptions.ClientError: An error occurred (UnknownService) when calling the InitiateAuth operation: no emulated service is registered for 'awscognitoidentityproviderservice'` (cloudtrail__cognito-idp.cognito-secret-request-parameters-are-hidden-in-the-delivered-record).

Contrast, same service, same run: cloudtrail__cognito-idp.obs-cognito-idp-3-failed-and-denied-admin-get-user-calls-are-audited-with-their-error-code gets a SUCCESSFUL AdminGetUser response from the emulator (it then fails on a separate, unrelated missing-RequestId defect), so the cognito-idp service itself is mounted and reachable for signed calls.

## Steps to Reproduce
1. Create a user pool and an app client with USER_PASSWORD_AUTH enabled; create and confirm a user (signed calls, which the emulator serves).
2. Call `cognito-idp.initiate_auth(ClientId=<app client>, AuthFlow='USER_PASSWORD_AUTH', AuthParameters={'USERNAME': ..., 'PASSWORD': ...})` — botocore sends this WITHOUT SigV4 credentials.
   Expect: HTTP 200 with an InitiateAuthResponse (or a modeled NotAuthorizedException).
   Observe: `UnknownService` / `no emulated service is registered for 'awscognitoidentityproviderservice'`.
3. Call any signed operation (e.g. `admin_get_user`) against the same emulator to confirm the service is otherwise reachable.
