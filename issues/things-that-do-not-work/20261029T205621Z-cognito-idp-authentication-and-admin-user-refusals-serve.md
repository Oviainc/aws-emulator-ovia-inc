Issue-Source: trace-sweep-divergence
Issue-Trace: cip-autherr-enabled-ghost-not-authorized
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cip-autherr-enabled-ghost-not-authorized cognito-idp-admin-user-auth-workflow cognito-idp-self-service-signup-and-token-session cip-autherr-legacy-ghost-user-not-found cip-autherr-forgot-password-ghost-legacy-not-found cip-cfg-admin-create-user-resend-confirmed-user cip-cfg-case-insensitive-username-resolution cognito-idp-admin-user-core-lifecycle cognito-idp-admin-user-group-and-account-linking-lifecycle cognito-idp-admin-user-signup-confirm-and-mfa cognito-idp-self-service-account-deletion-lifecycle cognito-idp-user-session-mfa-and-refresh cognito-idp-invalid-inputs-and-missing-resources cognito-idp-self-service-ops-reject-invalid-access-token
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|AdminInitiateAuth|4xx->4xx|status
Issue-Key: cognito-idp|ForgotPassword|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|ForgotPassword|4xx->4xx|status
Issue-Key: cognito-idp|AdminCreateUser|4xx->4xx|status
Issue-Key: cognito-idp|AdminGetUser|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|AdminGetUser|4xx->4xx|status
Issue-Key: cognito-idp|AdminConfirmSignUp|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|AdminConfirmSignUp|4xx->4xx|status
Issue-Key: cognito-idp|AdminSetUserSettings|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|AdminSetUserSettings|4xx->4xx|status
Issue-Key: cognito-idp|SetUserSettings|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|SetUserSettings|4xx->4xx|status
Issue-Key: cognito-idp|DeleteUser|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|DeleteUser|4xx->4xx|status
Issue-Related: 20261029T184956Z-cognito-idp-every-error-response-omits-the-x.md
Issue-Related: 20261029T184956Z-cognito-idp-ten-duplicate-resource-and-policy-refusal.md
Issue-Commit: 4bd656ceb6ae5ffdb198db72ede94ac7bf108594

# cognito-idp authentication and admin user refusals serve 404/403 instead of 400

> _Note: this issue was identified against the code base at commit `4bd656ceb6ae5ffdb198db72ede94ac7bf108594`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminInitiateAuth

## Description
This bundle has 10 causes; all 10 must be closed before submitting.

AWS: cognito-idp (awsJson1_1) answers every authentication and admin-user client-error class with HTTP status 400, carrying the class in the body's `__type` and in the `x-amzn-errortype` response header (recorded value: the error shape name followed by a colon, e.g. `UserNotFoundException:`). Smithy: UserNotFoundException @httpError(404), NotAuthorizedException @httpError(403), ResourceNotFoundException @httpError(404) — the recorded service does NOT apply those traits; recorded behavior only: every refusal on these paths has status 400. The message bodies matched on all ten steps, so only the status line and the error-type header diverge.

Emulator: the emulator applies the modeled @httpError, so user-not-found and resource-not-found refusals on auth/admin-user operations come back as 404 and not-authorized refusals as 403, and no response carries `x-amzn-errortype`.

Why: sign-in and user-administration clients branch on the HTTP status before they look at the body. A 403 from AdminInitiateAuth/AdminConfirmSignUp/SetUserSettings is read as a credential or IAM failure, so SDK and middleware credential-refresh/retry logic fires instead of surfacing the authentication refusal to the user; a 404 is read as 'operation or endpoint missing', so unknown-user handling (and PreventUserExistenceErrors-aware handling) never runs. Hosted sign-in pages and API gateways in front of Cognito reject or rewrite these statuses before the application sees them.

Fix: the cognito-idp awsJson1_1 error serializer and its error-to-HTTP-status mapping in the service crate — pin client errors to 400 instead of reading @httpError off the error shape, and emit `x-amzn-errortype: <ErrorShapeName>:` from the same serializer. The individual auth handlers already choose the right error class, so no handler logic needs to change for these steps.

Related open issues that do NOT cover these steps: 20261029T184956Z-cognito-idp-modeled-httperror-codes-are-served-where.md (ten other operations) and 20261029T184956Z-cognito-idp-every-error-response-omits-the-x.md (a different set of replay keys).

Cause 1 of 10: AdminInitiateAuth for a user that cannot be authenticated answers 403; AWS answers 400 NotAuthorizedException. Fixture cip-autherr-enabled-ghost-not-authorized, step task#9 (3 recorded steps in this cluster).
Cause 2 of 10: AdminInitiateAuth for a username that does not exist in the pool answers 404; AWS answers 400. Fixture cip-autherr-legacy-ghost-user-not-found, step task#9.
Cause 3 of 10: ForgotPassword for a username that does not exist answers 404; AWS answers 400. Fixture cip-autherr-forgot-password-ghost-legacy-not-found, step task#5.
Cause 4 of 10: AdminCreateUser with RESEND for a user that is already confirmed answers 404; AWS answers 400. Fixture cip-cfg-admin-create-user-resend-confirmed-user, step task#6.
Cause 5 of 10: AdminGetUser for a username that does not resolve answers 404; AWS answers 400. Fixture cip-cfg-case-insensitive-username-resolution, step task#11 (6 recorded steps).
Cause 6 of 10: AdminConfirmSignUp on a user whose state forbids it answers 403; AWS answers 400. Fixture cognito-idp-admin-user-signup-confirm-and-mfa, step task#7.
Cause 7 of 10: AdminConfirmSignUp for a username that does not exist answers 404; AWS answers 400. Fixture cognito-idp-admin-user-signup-confirm-and-mfa, step task#8.
Cause 8 of 10: AdminSetUserSettings for a username that does not exist answers 404; AWS answers 400 UserNotFoundException. Fixture cognito-idp-invalid-inputs-and-missing-resources, step task#3.
Cause 9 of 10: SetUserSettings with an access token that is not valid answers 403; AWS answers 400 NotAuthorizedException. Fixture cognito-idp-invalid-inputs-and-missing-resources, step task#5 (2 recorded steps).
Cause 10 of 10: DeleteUser after the user has been deleted answers 404; AWS answers 400 UserNotFoundException. Fixture cognito-idp-self-service-account-deletion-lifecycle, step task#10.

## Evidence
Expected (recorded AWS) for all ten clusters: HTTP 400, error class in body `__type`, header `x-amzn-errortype: <ErrorShapeName>:`.
Actual (emulator):
- AdminInitiateAuth (not authorized): `status: expected 400, got 403`; `$headers.x-amzn-errortype: missing`
- AdminInitiateAuth (unknown user): `status: expected 400, got 404`; header missing
- ForgotPassword: `status: expected 400, got 404`; header missing
- AdminCreateUser: `status: expected 400, got 404`; header missing
- AdminGetUser: `status: expected 400, got 404`; header missing
- AdminConfirmSignUp: `status: expected 400, got 403` and `status: expected 400, got 404`; header missing
- AdminSetUserSettings: `status: expected 400, got 404`; header missing (expected 'UserNotFoundException:')
- SetUserSettings: `status: expected 400, got 403`; header missing (expected 'NotAuthorizedException:')
- DeleteUser: `status: expected 400, got 404`; header missing (expected 'UserNotFoundException:')
Smithy: UserNotFoundException @httpError(404), NotAuthorizedException @httpError(403), ResourceNotFoundException @httpError(404) — not honored by the recorded service.
Fixed vs variable: status 400 and the error class name are the requirement; the username, pool id and client id that appear inside the message are taken from the request/state and are not.

## Steps to Reproduce
1. Replay cip-autherr-enabled-ghost-not-authorized step task#9 (AdminInitiateAuth that AWS refuses as NotAuthorizedException): emulator 403, AWS 400.
2. Replay cognito-idp-self-service-account-deletion-lifecycle step task#10 (DeleteUser for an already-deleted user): emulator 404, AWS 400 UserNotFoundException.
3. Check response headers on both: `x-amzn-errortype` is absent; AWS sends the error shape name plus a colon.
4. Repeat for each fixture#step listed under the causes.
