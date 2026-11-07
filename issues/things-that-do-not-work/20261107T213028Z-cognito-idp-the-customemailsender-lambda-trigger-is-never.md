Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.user-attribute-verification-flow
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.user-attribute-verification-flow|next_code#39|AssertionError|raise AssertionError(
Issue-Commit: 2d6e0284801305ae3aa8ad9d3ca49cea550d31a4

# cognito-idp: the CustomEmailSender Lambda trigger is never invoked by UpdateUserAttributes / GetUserAttributeVerificationCode, so no verification code is ever produced and the attribute-verification state machine is unreachable

> _Note: this issue was identified against the code base at commit `2d6e0284801305ae3aa8ad9d3ca49cea550d31a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateUserAttributes

## Description
One net-new root cause in the cognito-idp user-attribute verification path.

Cause 1 of 1: a user pool whose LambdaConfig carries a CustomEmailSender trigger (plus LambdaConfig.KMSKeyID) accepts that configuration and then behaves as if no trigger were configured: neither UpdateUserAttributes on a verifiable attribute nor GetUserAttributeVerificationCode invokes the function, so the generated code is never handed to the only sanctioned observer of it and the pending -> code -> verified transition can never be completed by a caller.
Failing check: cognito-idp.user-attribute-verification-flow

  AWS:      Smithy (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice): LambdaConfigType.CustomEmailSender is a CustomEmailLambdaVersionConfigType with required LambdaVersion (enum CustomEmailSenderLambdaVersionType, V1_0) and required LambdaArn, accepted on CreateUserPool/UpdateUserPool alongside LambdaConfig.KMSKeyID, and documented as "The configuration of a custom email sender Lambda trigger. This trigger routes all email notifications from a user pool to a Lambda function". UpdateUserAttributes (input UserAttributes required, AccessToken required; output CodeDeliveryDetailsList) and GetUserAttributeVerificationCode (AccessToken + AttributeName required; output CodeDeliveryDetails) both model the Lambda-interaction error set (UnexpectedLambdaException, InvalidLambdaResponseException, UserLambdaValidationException, all HTTP 400), which is the model stating that these operations invoke a configured trigger. Recorded behavior: with that trigger configured, both operations invoke the function INSTEAD of sending mail, with triggerSource CustomEmailSender_UpdateUserAttribute and CustomEmailSender_VerifyUserAttribute respectively, carrying request.code as an AWS-Encryption-SDK message encrypted under LambdaConfig.KMSKeyID; the decrypted plaintext is a 6-digit code. VerifyUserAttribute then refuses a wrong code with HTTP 400 CodeMismatchException — fixed text "Invalid verification code provided, please try again.", no variable part — and, with the real code, flips <attribute>_verified to "true".
  Emulator: the configured function is never invoked. The check's CustomEmailSender Lambda ships every decrypted code it receives to an SQS queue; that queue receives nothing at all for the whole run deadline. The replay reports `AssertionError: no ('CustomEmailSender_UpdateUserAttribute',) code for '<username>' reached the trigger's queue within the shared check deadline; deliveries seen but unmatched: []` — the empty "deliveries seen" list is the point: zero invocations, not a wrong triggerSource and not a mis-encrypted payload. The caller gets a normal 200 with CodeDeliveryDetails pointing at a destination nothing was ever sent to.
  Why:      an application that uses a custom sender trigger — the documented way to deliver codes over its own channel, or to observe/pin them in a test — gets silence: the pool reports the trigger in DescribeUserPool and is inert. The entire add-or-change-a-verifiable-attribute state machine (pending value -> code -> verified) becomes unreachable, and a client cannot distinguish "trigger configured but never fires" from "delivery still in flight", so it waits forever instead of failing.
  Fix:      the user-pool Lambda-trigger dispatch path for the CustomEmailSender trigger (invoke the configured LambdaArn with a customEmailSenderRequestV1 event whose request.code is the issued code encrypted under LambdaConfig.KMSKeyID, and whose triggerSource is CustomEmailSender_UpdateUserAttribute / CustomEmailSender_VerifyUserAttribute), together with the code-issuance/delivery state shared by the UpdateUserAttributes and GetUserAttributeVerificationCode handlers. The same inert-trigger seam is visible in the sibling handler services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/forgot_password.rs:237, which states outright that this emulator invokes none of the Lambda triggers an action would invoke.
  Related open issues (neither carries this check's current replay key, so neither covers it): 20261025T062108Z-cognito-idp-two-causes-verifyuserattribute-answers-501-notimplemented.md lists this check id but is keyed on an older timeout signature at next_code, and 20261025T015335Z-cognito-idp-kms-lambda-sqs-the-customemailsender-lambda.md reports the same trigger inert for the cognito-idp__kms__lambda__sqs checks. All should be fixed together.
  Traces:   cognito-idp.user-attribute-verification-flow.

## Evidence
Check cognito-idp.user-attribute-verification-flow — verdict [mismatch].

Actual (replay output):
  File "check_cognito_idp_user_attribute_verification_flow.py", line 229, in run
    real_code = inbox.next_code(wrong_user, (_UPDATE_SOURCE,), run_deadline)
  File "check_cognito_idp_user_attribute_verification_flow.py", line 630, in next_code
    raise AssertionError(
  AssertionError: no ('CustomEmailSender_UpdateUserAttribute',) code for '<username>' reached the trigger's queue within the shared check deadline; deliveries seen but unmatched: []

Expected: after CreateUserPool/UpdateUserPool with LambdaConfig {CustomEmailSender {LambdaVersion: "V1_0", LambdaArn: <fn>}, KMSKeyID: <key arn>}, an UpdateUserAttributes changing the email attribute invokes <fn> with triggerSource "CustomEmailSender_UpdateUserAttribute" and request.code = the ESDK-encrypted 6-digit code; a GetUserAttributeVerificationCode resend invokes it again (triggerSource CustomEmailSender_VerifyUserAttribute or CustomEmailSender_UpdateUserAttribute) with a fresh code; VerifyUserAttribute with a deliberately-wrong code is refused HTTP 400 Error.Code=CodeMismatchException, Error.Message="Invalid verification code provided, please try again." (fixed text, no variable part), and with the delivered code succeeds and sets email_verified="true".

Actual: zero invocations of the configured function — the sink the trigger writes to stays empty for the entire run deadline — so no code is ever observable and no leg of the verify flow can be exercised. Usernames, queue URLs, key ARNs and the code digits above are per-run values, not requirements.

## Steps to Reproduce
1. Create a symmetric KMS key whose policy lets cognito-idp.amazonaws.com Encrypt and CreateGrant; create an SQS queue; create a Lambda that AWS-Encryption-SDK-decrypts event.request.code and forwards {username, triggerSource, plaintext code} to the queue; grant lambda:InvokeFunction to principal cognito-idp.amazonaws.com for that function.
2. CreateUserPool (or UpdateUserPool) with LambdaConfig = {CustomEmailSender: {LambdaVersion: "V1_0", LambdaArn: <fn arn>}, KMSKeyID: <key arn>} and AutoVerifiedAttributes=["email"]; CreateUserPoolClient with ALLOW_ADMIN_USER_PASSWORD_AUTH.
3. AdminCreateUser + AdminSetUserPassword(Permanent) a user, AdminInitiateAuth to obtain its AccessToken.
4. Call UpdateUserAttributes(AccessToken=<token>, UserAttributes=[{Name: "email", Value: <new address>}]) and poll the SQS queue.
   Expected: one invocation with triggerSource "CustomEmailSender_UpdateUserAttribute" delivering a 6-digit code.
   Observed against the emulator: no invocation; the queue stays empty.
5. Call GetUserAttributeVerificationCode(AccessToken=<token>, AttributeName="email") and poll again — again nothing arrives, so VerifyUserAttribute can never be driven with a real code.
