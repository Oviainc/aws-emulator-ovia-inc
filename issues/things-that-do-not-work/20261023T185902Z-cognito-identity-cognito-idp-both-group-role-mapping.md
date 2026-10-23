Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity__cognito-idp.deep-cognito-identity-3-token-mapping-ambiguous-multi-role-claim-fallback-deny-and-custom-role cognito-identity__cognito-idp.group-precedence-selects-the-vended-role-and-flips-with-precedence
Issue-Key: cognito-identity__cognito-idp.deep-cognito-identity-3-token-mapping-ambiguous-multi-role-claim-fallback-deny-and-custom-role|_poll_until#35|Timeout|time.sleep(interval_s)
Issue-Key: cognito-identity__cognito-idp.group-precedence-selects-the-vended-role-and-flips-with-precedence|_poll_until#29|Timeout|time.sleep(interval_s)
Issue-Commit: 144a2fce0f9e3116760062dd946ebab581523079

# cognito-identity__cognito-idp: both group/role-mapping token checks hang to the 60 s harness cap because the polled sign-in and credential-vend calls never succeed (1 cause)

> _Note: this issue was identified against the code base at commit `144a2fce0f9e3116760062dd946ebab581523079`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
One cause (Cause 1 of 1).

AWS: the user-pool sign-in (InitiateAuth) returns an ID token whose cognito:groups/cognito:roles claims reflect group membership, and cognito-identity GetCredentialsForIdentity then vends a role according to the pool's RoleMapping (Type=Token with AmbiguousRoleResolution, or group precedence). Each of these calls answers within one request — the checks poll only to absorb IAM role-propagation delay, not to wait for the service to come up.
Emulator: both checks make no progress and are killed by the harness at the 60 s cap ("Error: Check timed out after 60.0s"). The polling helpers in these checks swallow every ClientError as "not yet", so a persistently refused call is observed as a hang rather than an error. In this same suite run the two calls these checks poll are refused on every attempt: cognito-idp InitiateAuth answers error code UnknownService "no emulated service is registered for 'awscognitoidentityproviderservice'", and unsigned cognito-identity GetId answers UnknownService "no emulated service is registered for 'awscognitoidentityservice'".
Hypothesis: the timeout is those persistent UnknownService refusals being retried inside _poll_until until the cap, not a slow but converging emulator. Confirm by re-replaying either check and logging the swallowed ClientError.
Why: a client blocks for the whole timeout budget instead of receiving an error, so group-driven role selection appears to hang rather than fail; it also burns 60 s of every suite run per check.
Fix: route the refused calls (the unsigned/anonymous dispatch in the request router and the cognito-idp auth operations) so the polled calls return a real answer; whatever remains must at minimum surface a modeled error to the caller instead of nothing.
Traces: cognito-identity__cognito-idp.deep-cognito-identity-3-token-mapping-ambiguous-multi-role-claim-fallback-deny-and-custom-role, cognito-identity__cognito-idp.group-precedence-selects-the-vended-role-and-flips-with-precedence.

## Evidence
Replay verdicts:
  [mismatch] cognito-identity__cognito-idp.deep-cognito-identity-3-token-mapping-ambiguous-multi-role-claim-fallback-deny-and-custom-role -> Error: Check timed out after 60.0s
  [mismatch] cognito-identity__cognito-idp.group-precedence-selects-the-vended-role-and-flips-with-precedence -> Error: Check timed out after 60.0s
Both checks drive their work through _poll_until(...) / _vend(...) loops whose except-branch records ClientError as "not yet" (source shown in the report), so a permanently refused call cannot surface as an error code.
In the same run the two operations they poll are refused on every call:
  An error occurred (UnknownService) when calling the InitiateAuth operation: no emulated service is registered for 'awscognitoidentityproviderservice'
  An error occurred (UnknownService) when calling the GetId operation: no emulated service is registered for 'awscognitoidentityservice'
Expected: sign-in returns an ID token with cognito:roles for both groups, and GetCredentialsForIdentity vends the pool default role (AmbiguousRoleResolution=AuthenticatedRole), refuses with NotAuthorizedException under Deny, and honours a CustomRoleArn present in cognito:roles.

## Steps to Reproduce
1. Create a user pool + app client, two groups each with a distinct RoleArn, and a user in both groups.
2. Create an identity pool wired to that user pool provider and set Roles + a RoleMappings entry of Type=Token.
3. Call cognito-idp InitiateAuth for the user -> expect an ID token; observe UnknownService for 'awscognitoidentityproviderservice'.
4. Call cognito-identity GetId/GetCredentialsForIdentity with that login -> expect an IdentityId and vended credentials; observe UnknownService for 'awscognitoidentityservice'.
5. Run either listed check under the behavioral harness: it polls those calls and is killed at the 60 s cap.
