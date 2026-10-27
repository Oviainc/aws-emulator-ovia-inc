Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.code-delivery-failure-error cognito-idp.code-mismatch-vs-expired-precision cognito-idp.op-contract-confirmSignUp
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.code-delivery-failure-error|run#11|AssertionError|assert code1 == "InvalidParameterException", (
Issue-Key: cognito-idp.code-mismatch-vs-expired-precision|run#19|AssertionError|assert c4 == "ExpiredCodeException", (
Issue-Key: cognito-idp.op-contract-confirmSignUp|run#15|AssertionError|assert code1.get("Message") == "Username/client id combination not found.", code1.get("Message")
Issue-Commit: 21bca675a34dab10ee37fe0445fa59a838d6e2c4

# cognito-idp: three causes — AdminCreateUser accepts an SMS-only delivery request for a user with no phone_number, ConfirmSignUp conflates 'no code outstanding' with a code mismatch, and its UserNotFoundException uses the admin-scoped message on a client-scoped operation

> _Note: this issue was identified against the code base at commit `21bca675a34dab10ee37fe0445fa59a838d6e2c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ConfirmSignUp

## Description
Three distinct root causes in the cognito-idp emulator, each grounded in a failing behavioral check that this replay did NOT mark 'ALREADY FILED' or 'OWNED by'. All three must be closed before this item is submitted.

Note on provenance: two of these three check ids appear in the Issue-Trace-Ids of the still-open 20261025T030229Z-cognito-idp-six-causes-confirmsignup-resendconfirmationcode-getuserattributeverificationcode-updateuserpooldomain.md, but at DIFFERENT replay keys (that issue carries them at `assert c1 == "CodeMismatchException"` and `assert resend_code in (None, "LimitExceededException")`, both of which were NotImplemented refusals that no longer reproduce). ConfirmSignUp and ResendConfirmationCode now answer, and each check has advanced to a new, further assertion — the causes below. They are net-new and are not covered by that issue.

=== Cause 1 of 3: AdminCreateUser accepts DesiredDeliveryMediums=['SMS'] for a user that has no phone_number attribute ===
AWS: Smithy AdminCreateUser — input DesiredDeliveryMediums targets DeliveryMediumListType, and the operation models InvalidParameterException (errorKind client, httpError 400) alongside CodeDeliveryFailureException (400). Requesting SMS delivery of the temporary password for a user whose UserAttributes carry no phone_number is caught at request-VALIDATION time with InvalidParameterException / HTTP 400, before any send is attempted, and no user is created (recorded behavior: the check's oracle observes the validation refusal, not a delivery failure — i.e. the refusal is InvalidParameterException, not CodeDeliveryFailureException).
Emulator: the call SUCCEEDS. The check's `_err()` helper returns None, meaning AdminCreateUser answered HTTP 200 and created the user with an SMS-only delivery medium it can never satisfy.
Why: this is an over-permissive acceptance. An application that provisions users with DesiredDeliveryMediums=['SMS'] but never collects phone_number appears correctly wired against the emulator, while against real AWS every such AdminCreateUser is a 400 — the failure surfaces only in production. Callers that branch on InvalidParameterException to fall back to EMAIL never take that branch.
Fix: AdminCreateUser's input validation in the cognito-idp handler (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/admin_create_user.rs) — cross-validate each requested delivery medium against the presence of the attribute that medium needs (SMS -> phone_number, EMAIL -> email) and refuse with InvalidParameterException / 400 before any state is written. The same validation is asserted for GetUserAttributeVerificationCode later in this check (AttributeName='phone_number' on a user with no phone_number must also be InvalidParameterException / 400), so it belongs in the shared delivery-medium validation path.
Traces: cognito-idp.code-delivery-failure-error.

=== Cause 2 of 3: ConfirmSignUp answers CodeMismatchException where no confirmation code was ever issued; AWS answers ExpiredCodeException ===
AWS: Smithy ConfirmSignUp models BOTH CodeMismatchException (errorKind client, httpError 400) and ExpiredCodeException (errorKind client, httpError 400) as distinct errors. The two are not interchangeable: a wrong code checked against a code that IS outstanding is CodeMismatchException, while a code checked where NONE was ever issued — e.g. a SignUp on a pool with nothing to auto-verify, so no delivery ever occurred and the response carries no CodeDeliveryDetails — is ExpiredCodeException, because there is no live code to compare against. The recorded message for that condition is 'Invalid code provided, please request a code again.' (as against 'Invalid verification code provided, please try again.' for the mismatch), so the two conditions are distinguishable on both code and message.
Emulator: ConfirmSignUp(ClientId=..., Username=<user signed up with no auto-verified attribute>, ConfirmationCode='000000') is refused with CodeMismatchException instead of ExpiredCodeException.
Why: the client cannot tell 'your code was wrong, try again' from 'there is no code, request one' — the two have opposite remediations. A sign-up UI that retries on CodeMismatchException and only calls ResendConfirmationCode on ExpiredCodeException loops forever against this emulator, and a target that always answers one of the two passes every mismatch check while being wrong about half the contract.
Fix: the confirmation-code comparison in the ConfirmSignUp handler (.../src/ops/confirm_sign_up.rs) and the per-user code state it consults — the 'no outstanding code' branch must be decided BEFORE the comparison and must raise ExpiredCodeException. The same distinction is asserted across ConfirmForgotPassword, VerifyUserAttribute and VerifySoftwareToken in this check, so it belongs in the shared code-verification path. The user must stay UNCONFIRMED either way.
Traces: cognito-idp.code-mismatch-vs-expired-precision.

=== Cause 3 of 3: ConfirmSignUp's UserNotFoundException carries the admin-scoped message instead of the client-scoped one ===
AWS: Smithy ConfirmSignUp is a client-id-scoped operation (required ClientId, optionalAuth/anonymous) and models UserNotFoundException (errorKind client, httpError 404). Because the lookup is of a (username, client id) PAIR rather than of a user inside a pool the caller is authorized on, AWS builds the fixed, request-independent message 'Username/client id combination not found.' for this operation — distinct from the admin-scoped lookup path (AdminGetUser, AdminAddUserToGroup, ...), whose fixed message is 'User does not exist.'. Both strings are fixed for their condition and interpolate nothing from the request, so the rule that produces each is known and the message is a requirement, not an example.
Emulator: the status (404) and the error code (UserNotFoundException) are already correct, but the message is 'User does not exist.' — the admin-scoped string, returned from a client-scoped operation.
Why: the two messages are what tells a caller which half of the lookup failed; the client-scoped wording deliberately does not assert anything about the user's existence on its own, because the pair may have failed on the client id. Clients and log scrubbers that match the documented client-scoped string stop matching, and the emulator's answer now claims more about user existence than AWS's does on an anonymous, unauthenticated code path.
Fix: the UserNotFoundException message construction for the client-id-scoped lookup in ConfirmSignUp (.../src/ops/confirm_sign_up.rs) and the resolver it shares with the other ClientId-scoped operations (ForgotPassword, ResendConfirmationCode, ConfirmForgotPassword, InitiateAuth). The resolved issue 20261024T114943Z-cognito-idp-three-error-messages-are-not-the.md fixed the ADMIN-path message to the fixed string 'User does not exist.'; that string now appears to be returned from the client-scoped path too, so the fix needs to be split by lookup scope rather than reverted.
Traces: cognito-idp.op-contract-confirmSignUp.

## Evidence
Cause 1 — cognito-idp.code-delivery-failure-error, frame `check_cognito_idp_code_delivery_failure_error.py`, line 161:
  AssertionError: AdminCreateUser: expected InvalidParameterException, got None
The call under test is `admin_create_user(UserPoolId=<pool with AutoVerifiedAttributes=['email']>, Username=<new>, DesiredDeliveryMediums=['SMS'], UserAttributes=[{'Name': 'email', 'Value': '<...>@example.com'}])` — no phone_number anywhere. `None` is the helper's value for 'the call raised no ClientError', i.e. HTTP 200. Expected HTTP 400 / InvalidParameterException with no user created.

Cause 2 — cognito-idp.code-mismatch-vs-expired-precision, frame `check_cognito_idp_code_mismatch_vs_expired_precision.py`, line 223:
  AssertionError: ConfirmSignUp with no code ever issued: expected ExpiredCodeException (no live code to compare against), got CodeMismatchException
Set-up in the same run: `sign_up(ClientId=..., Username=<ne_user>, Password=...)` with no auto-verified attribute, asserted to return no CodeDeliveryDetails, then `confirm_sign_up(ClientId=..., Username=<ne_user>, ConfirmationCode='000000')`. Expected HTTP 400 ExpiredCodeException (message example 'Invalid code provided, please request a code again.'); actual HTTP 400 CodeMismatchException. The earlier leg of the same check — a wrong code against an OUTSTANDING code — is the positive control and must stay CodeMismatchException.

Cause 3 — cognito-idp.op-contract-confirmSignUp, frame `check_cognito_idp_op_contract_confirmSignUp.py`, line 59:
  AssertionError: User does not exist.
  (the assertion is `code1.get('Message') == 'Username/client id combination not found.'`)
The call is `confirm_sign_up(ClientId=<real client>, Username='no-such-user-xyz', ConfirmationCode='000000')`. The preceding assertion on the error CODE (UserNotFoundException) PASSED, so only the message diverges; HTTP status 404 is already correct.

## Steps to Reproduce
Boot the emulator from this checkout and drive the cognito-idp endpoint directly, or replay the three check ids in trace_ids.

Cause 1: CreateUserPool(AutoVerifiedAttributes=['email'], Policies.PasswordPolicy.MinimumLength=8, DeletionProtection='INACTIVE') + CreateUserPoolClient(GenerateSecret=false). Then AdminCreateUser(UserPoolId=<pool>, Username=<new>, DesiredDeliveryMediums=['SMS'], UserAttributes=[{'Name':'email','Value':'x@example.com'}]) — expect HTTP 400 InvalidParameterException and no user created; observe HTTP 200 and a created user. Positive control: the same call with DesiredDeliveryMediums=['EMAIL'] must still succeed. Same validation via the token path: sign a user in, then GetUserAttributeVerificationCode(AccessToken=..., AttributeName='phone_number') on a user with no phone_number — expect 400 InvalidParameterException, while AttributeName='email' on a user that has one must succeed.

Cause 2: on that pool (no auto-verified attribute for the test user's path), SignUp(ClientId=..., Username=<u>, Password=...) with NO email/phone attribute — the response must carry no CodeDeliveryDetails. Then ConfirmSignUp(ClientId=..., Username=<u>, ConfirmationCode='000000') — expect HTTP 400 ExpiredCodeException; observe CodeMismatchException. Contrast with a user signed up on an auto-verified email (a code IS outstanding): a wrong code there must be CodeMismatchException. In both cases AdminGetUser must still report UserStatus='UNCONFIRMED'.

Cause 3: CreateUserPool + CreateUserPoolClient, then ConfirmSignUp(ClientId=<that client>, Username='no-such-user-xyz', ConfirmationCode='000000') — expect HTTP 404 UserNotFoundException with message 'Username/client id combination not found.'; observe the same status and code with message 'User does not exist.'. Contrast: AdminGetUser(UserPoolId=..., Username='no-such-user-xyz') must keep answering 'User does not exist.'.
