Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.alias-exists-on-verify-attribute cognito-idp.user-attribute-verification-flow
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.alias-exists-on-verify-attribute|run#13|AssertionError|assert code == "CodeMismatchException", (
Issue-Key: cognito-idp.user-attribute-verification-flow|next_code#39|Timeout|resp = self.sqs.receive_message(
Issue-Commit: 9a7a747cd70d8081df9aaa46247c8067ae6a4019

# cognito-idp: two causes — VerifyUserAttribute answers 501 NotImplemented, and the CustomEmailSender trigger is never invoked so the attribute-verification flow hangs

> _Note: this issue was identified against the code base at commit `9a7a747cd70d8081df9aaa46247c8067ae6a4019`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
VerifyUserAttribute

## Description
Two distinct root causes in the cognito-idp user-attribute verification path, both grounded in failing behavioral checks of this suite. Both must be closed before submitting.

Cause 1 of 2: VerifyUserAttribute is an unimplemented scaffold stub, so every attribute-verification attempt is refused with 501 NotImplemented instead of the modeled outcome.
Failing check: cognito-idp.alias-exists-on-verify-attribute
  AWS:      Smithy model (com-amazonaws-cognitoidentityprovider): operation VerifyUserAttribute takes AccessToken (required, sensitive), AttributeName (required, len 1..32) and Code (required, len 1..2048) and returns an empty 200 output. Its modeled errors include CodeMismatchException (HTTP 400, 'This exception is thrown if the provided code doesn't match what the server was expecting'), ExpiredCodeException (400), AliasExistsException (400), LimitExceededException (400), NotAuthorizedException (403) and UserNotFoundException (404). Recorded behavior: after UpdateUserAttributes leaves a pending phone_number change whose value is already a VERIFIED alias of another user, VerifyUserAttribute with a wrong code is refused with error code CodeMismatchException at HTTP 400 — AWS checks the code BEFORE the alias — and the attribute is left unverified (phone_number_verified absent or 'false'). The recorded message 'Invalid verification code provided, please try again.' is an example, not a requirement.
  Emulator: the request is refused with HTTP 501 and error code NotImplemented ('operation VerifyUserAttribute is scaffolded but not implemented in this emulator'); the handler returns Scaffolded::not_implemented unconditionally, so no code check, no alias check, and no state transition happen at all.
  Why:      a client can never complete a verifiable-attribute change: email/phone verification simply cannot be finished, so any sign-up/profile-update flow that depends on a verified alias is dead. Worse for callers that handle refusals: they expect a modeled 400 CodeMismatchException/ExpiredCodeException to drive a retry prompt and get an unmodeled 501 that SDK error handling treats as a server fault, and nothing in the emulator ever exercises the 'a guessed code must never silently verify someone else's alias' guarantee.
  Fix:      the VerifyUserAttribute operation handler and the user-attribute state it must mutate (pending vs verified attribute values, per-user outstanding verification code, alias index).
  Traces:   cognito-idp.alias-exists-on-verify-attribute; stub marker at services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/verify_user_attribute.rs:4 and the refusal at the same file line 205.

Cause 2 of 2: a user pool configured with a CustomEmailSender Lambda trigger never invokes it, so no verification code is ever produced and the whole attribute-verification state-machine check hangs to the harness cap.
Failing check: cognito-idp.user-attribute-verification-flow
  AWS:      Smithy model: UserPoolType/CreateUserPool/UpdateUserPool accept LambdaConfig.CustomEmailSender (CustomEmailLambdaVersionConfigType with required LambdaVersion — enum CustomEmailSenderLambdaVersionType, V1_0 — and required LambdaArn) together with LambdaConfig.KMSKeyID, documented as 'The configuration of a custom email sender Lambda trigger. This trigger routes all email notifications from a user pool to a Lambda function...'. Recorded behavior: with that trigger configured, UpdateUserAttributes on a verifiable attribute and GetUserAttributeVerificationCode invoke the function INSTEAD of sending an email, passing the generated code in request.code (an AWS Encryption SDK message encrypted under LambdaConfig.KMSKeyID) with triggerSource CustomEmailSender_UpdateUserAttribute / CustomEmailSender_VerifyUserAttribute; the function is the only sanctioned way a caller observes the real code.
  Emulator: the trigger function is never invoked — the SQS sink the trigger would ship the decrypted code to stays empty — so the check blocks waiting for a code it can never observe and is killed by the harness at the 60 s cap ('Check timed out after 60.0s'). Nothing is returned to the caller at all.
  Why:      any application that relies on custom sender triggers (to deliver codes over its own channel, or to observe/pin them) gets silence: the pool accepts the LambdaConfig and then behaves as if no trigger were configured, and the whole add/verify-attribute state machine (pending value -> code -> verified) is unreachable and untestable. A caller cannot distinguish 'configured but inert' from 'delivery in flight', so it waits forever.
  Fix:      the user-pool Lambda-trigger dispatch path for the CustomEmailSender trigger (invoke the configured LambdaArn with the customEmailSenderRequestV1 event and the code encrypted under LambdaConfig.KMSKeyID) and the code-generation/delivery state shared by UpdateUserAttributes and GetUserAttributeVerificationCode. NOTE: open issue 20261025T015335Z-cognito-idp-kms-lambda-sqs-the-customemailsender-lambda.md reports the same trigger being inert for the cognito-idp__kms__lambda__sqs checks; it does not carry this check's replay key (cognito-idp.user-attribute-verification-flow), so this cause is filed here and the two should be fixed together.
  Traces:   cognito-idp.user-attribute-verification-flow.

## Evidence
Cause 1 — check cognito-idp.alias-exists-on-verify-attribute, mismatch:
  AssertionError: expected the pending, alias-colliding phone_number change with a guessed code to be rejected with CodeMismatchException (AWS checks the code before the alias) -- never silently verified, got NotImplemented
  Expected: HTTP 400, Error.Code = CodeMismatchException (example message 'Invalid verification code provided, please try again.'), then GetUser shows phone_number_verified in (absent, 'false').
  Actual:   HTTP 501, Error.Code = NotImplemented, message 'operation VerifyUserAttribute is scaffolded but not implemented in this emulator'.
  Handler: services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/verify_user_attribute.rs — line 4 carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker; line 205 is `Err(crate::scaffolded::Scaffolded::not_implemented("VerifyUserAttribute"))`.

Cause 2 — check cognito-idp.user-attribute-verification-flow, mismatch:
  Error: Check timed out after 60.0s
  Expected: after CreateUserPool/UpdateUserPool with LambdaConfig {CustomEmailSender {LambdaVersion 'V1_0', LambdaArn <fn>}, KMSKeyID <key>}, an UpdateUserAttributes on a verifiable attribute (and a GetUserAttributeVerificationCode resend) invokes that function with triggerSource CustomEmailSender_UpdateUserAttribute / CustomEmailSender_VerifyUserAttribute and request.code set to the ESDK-encrypted code, so the code becomes observable; VerifyUserAttribute then rejects a wrong code and accepts the correct one.
  Actual:   no invocation ever happens, the queue the trigger writes to receives nothing, and the check blocks on that wait until the harness kills it at 60 s.

## Steps to Reproduce
Cause 1 (cognito-idp.alias-exists-on-verify-attribute):
1. CreateUserPool with AliasAttributes=['phone_number'], AutoVerifiedAttributes=['phone_number'] and an SmsConfiguration naming an SNS caller role.
2. CreateUserPoolClient with ExplicitAuthFlows=['ALLOW_ADMIN_USER_PASSWORD_AUTH','ALLOW_REFRESH_TOKEN_AUTH'].
3. AdminCreateUser user A with phone_number=+1206555XXXX and phone_number_verified='true'; AdminSetUserPassword Permanent.
4. AdminCreateUser user B, AdminSetUserPassword Permanent, AdminInitiateAuth ADMIN_USER_PASSWORD_AUTH to get B's AccessToken.
5. UpdateUserAttributes(AccessToken=B, UserAttributes=[{phone_number: A's number}]) -> pending change with CodeDeliveryDetailsList.
6. VerifyUserAttribute(AccessToken=B, AttributeName='phone_number', Code='000000').
   Expected HTTP 400 CodeMismatchException; observed HTTP 501 NotImplemented.

Cause 2 (cognito-idp.user-attribute-verification-flow):
1. Create a symmetric KMS key whose policy lets cognito-idp.amazonaws.com Encrypt/CreateGrant; create an SQS queue; create a Lambda (nodejs22.x) that ESDK-decrypts request.code and sends the plaintext to the queue, with lambda:InvokeFunction granted to cognito-idp.amazonaws.com.
2. CreateUserPool/UpdateUserPool with LambdaConfig {CustomEmailSender {LambdaVersion 'V1_0', LambdaArn <fn>}, KMSKeyID <key arn>} and a client with ALLOW_ADMIN_USER_PASSWORD_AUTH.
3. Sign a confirmed user in, then UpdateUserAttributes to change the email attribute, and poll the SQS queue for the code.
   Expected: the trigger is invoked and the code arrives on the queue. Observed: no invocation, the queue stays empty and the check is killed at the 60 s cap.
