Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-email-otp-session-expiry-rejected
Issue-Key: cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-email-otp-session-expiry-rejected|next_code#6|Timeout|resp = self.sqs.receive_message(QueueUrl=self.queue_url, MaxNumberOfMessages=10,
Issue-Commit: 893f29be3e6a6715693c5d70a91ed2f1aa3a2caf

# cognito-idp__kms__lambda__sqs: the EMAIL_OTP session-expiry scenario never gets a real code — the CustomEmailSender trigger is never invoked, so no KMS-encrypted code is delivered and AuthSessionValidity cannot be exercised

> _Note: this issue was identified against the code base at commit `893f29be3e6a6715693c5d70a91ed2f1aa3a2caf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth

## Description
One net-new root cause in the cognito-idp__kms__lambda__sqs suite. Seven sibling checks of the same family are OWNED by open issue 20261025T015335Z-cognito-idp-kms-lambda-sqs-the-customemailsender-lambda.md, but THIS check is marked neither ALREADY FILED nor OWNED there, so it is filed here and that issue is named as the adjacent report.

Cause 1 of 1: a user pool with LambdaConfig.CustomEmailSender never invokes the trigger, so no verification/authentication code is ever produced and the real-code EMAIL_OTP flow cannot run at all.
AWS: Smithy (com.amazonaws.cognitoidentityprovider) models CreateUserPool.LambdaConfig.CustomEmailSender {LambdaVersion: V1_0, LambdaArn} with LambdaConfig.KMSKeyID, and InitiateAuth with AuthFlow USER_AUTH returning ChallengeName EMAIL_OTP plus a Session, answered by RespondToAuthChallenge(ChallengeName='EMAIL_OTP', ChallengeResponses={'USERNAME','EMAIL_OTP_CODE'}); NotAuthorizedException is modelled on RespondToAuthChallenge. The delivery itself is recorded behavior: for every code-bearing event (CustomEmailSender_SignUp, CustomEmailSender_Authentication, CustomEmailSender_ForgotPassword, ...) Cognito invokes the configured Lambda with the code encrypted to the pool's KMS key (an AWS Encryption SDK message in request.code, decryptable with kms:Decrypt under the message's own encryption context), the delivered email OTP is 8 digits, and the code's useful life is bounded by the app client's AuthSessionValidity — a code answered inside the window issues tokens, one answered after it is refused with Code `NotAuthorizedException` and Message `Invalid session for the user, session is expired.` (all fixed text) and no AuthenticationResult.
Emulator: the check hangs to the harness cap ("Check timed out after 60.0s") while draining the trigger's SQS sink for the first SignUp code — the CustomEmailSender Lambda is never invoked (nothing is published to the sink), so there is no decrypted 8-digit code to answer with and none of the session-window legs (early accept, late NotAuthorizedException refusal, fresh-code positive control) can be reached.
Why: every pool that customises code delivery is unusable: SignUp/ConfirmSignUp, EMAIL_OTP sign-in and password reset all depend on a code that is never delivered, and a client cannot complete or even observe the challenge. It also hides the session-expiry refusal, so an application that relies on AuthSessionValidity cannot be tested at all.
Fix: the CustomEmailSender trigger invocation in the cognito-idp crate's user-pool code-delivery path (and the lambda crate's ability to actually run the configured function) — invoke the configured LambdaArn for every code-bearing trigger source with the code encrypted to LambdaConfig.KMSKeyID, and enforce AuthSessionValidity on RespondToAuthChallenge with the NotAuthorizedException message above.
Traces: cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-email-otp-session-expiry-rejected.

## Evidence
cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-email-otp-session-expiry-rejected [mismatch]: Error: Check timed out after 60.0s.
The check stands up a KMS key, an SQS sink, an execution role, a Node.js CustomEmailSender function, a user pool with LambdaConfig={'CustomEmailSender': {'LambdaVersion':'V1_0','LambdaArn':<fn>}, 'KMSKeyID': <key arn>} and an app client with AuthSessionValidity=3, then calls sign_up and blocks in `inbox.next_code(user, 'CustomEmailSender_SignUp')`, which drains the sink for the trigger's decrypted code. It never returned, so no trigger invocation reached the sink.
Expected (AWS) past that point: an 8-digit decrypted code per delivery; an EMAIL_OTP code answered 30 s before the 3-minute session closes returns AuthenticationResult with an AccessToken; the same kind of real code answered 30 s after the window is refused with Error.Code 'NotAuthorizedException' and Error.Message 'Invalid session for the user, session is expired.' and no AuthenticationResult; and a fresh OTP then signs the late user in. The seven sibling real-code checks in this family failed as mismatches on the same missing trigger and are covered by 20261025T015335Z-cognito-idp-kms-lambda-sqs-the-customemailsender-lambda.md.

## Steps to Reproduce
1. kms:create_key with a policy allowing cognito-idp.amazonaws.com kms:Encrypt/kms:CreateGrant; sqs:create_queue as the sink; iam:create_role + inline kms:Decrypt/sqs:SendMessage; lambda:create_function (nodejs) that decrypts request.code and forwards the plaintext to the queue.
2. cognito-idp:create_user_pool with AutoVerifiedAttributes=['email'] and LambdaConfig={'CustomEmailSender':{'LambdaVersion':'V1_0','LambdaArn':<fn arn>},'KMSKeyID':<key arn>}; create an app client with AuthSessionValidity=3 and the USER_AUTH flow enabled.
3. cognito-idp:sign_up a user, then poll the sink queue for the trigger's message.
   Expected: one invocation per code-bearing event, carrying the KMS-encrypted code, yielding an 8-digit plaintext OTP. Actual: nothing is ever delivered (the check hangs to the harness cap).
4. (Blocked behind the above:) initiate_auth(AuthFlow='USER_AUTH', AuthParameters={'USERNAME':u,'PREFERRED_CHALLENGE':'EMAIL_OTP'}) must return ChallengeName='EMAIL_OTP' and a Session; respond_to_auth_challenge with the real code ~150 s in must return AuthenticationResult.AccessToken; the same for a second user ~210 s in (past the 3-minute window) must be refused with NotAuthorizedException 'Invalid session for the user, session is expired.' and no tokens; a fresh OTP must then sign that user in.
Replay cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-email-otp-session-expiry-rejected to confirm.
