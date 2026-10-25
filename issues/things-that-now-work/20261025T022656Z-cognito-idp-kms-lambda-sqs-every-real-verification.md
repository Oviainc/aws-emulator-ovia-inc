Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-email-otp-session-expiry-rejected cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-email-otp-wrong-code-rejected cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-forgot-password-code-reuse-rejected cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-forgot-password-confirm cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-reissued-reset-code-invalidates-first-rejected cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-resent-signup-code-invalidates-first-rejected cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-verify-email-wrong-code-rejected cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-verify-updated-email
Issue-Key: cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-email-otp-session-expiry-rejected|next_code#6|Timeout|resp = self.sqs.receive_message(QueueUrl=self.queue_url, MaxNumberOfMessages=10,
Issue-Key: cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-email-otp-wrong-code-rejected|next_code#6|Timeout|resp = self.sqs.receive_message(QueueUrl=self.queue_url, MaxNumberOfMessages=10,
Issue-Key: cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-forgot-password-code-reuse-rejected|next_code#6|Timeout|resp = self.sqs.receive_message(QueueUrl=self.queue_url, MaxNumberOfMessages=10,
Issue-Key: cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-forgot-password-confirm|next_code#6|Timeout|resp = self.sqs.receive_message(QueueUrl=self.queue_url, MaxNumberOfMessages=10,
Issue-Key: cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-reissued-reset-code-invalidates-first-rejected|next_code#6|Timeout|resp = self.sqs.receive_message(QueueUrl=self.queue_url, MaxNumberOfMessages=10,
Issue-Key: cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-resent-signup-code-invalidates-first-rejected|next_code#6|Timeout|resp = self.sqs.receive_message(QueueUrl=self.queue_url, MaxNumberOfMessages=10,
Issue-Key: cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-verify-email-wrong-code-rejected|next_code#7|Timeout|resp = self.sqs.receive_message(QueueUrl=self.queue_url, MaxNumberOfMessages=10,
Issue-Key: cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-verify-updated-email|next_code#7|Timeout|resp = self.sqs.receive_message(QueueUrl=self.queue_url, MaxNumberOfMessages=10,
Issue-Commit: f04105fb2faf5f6bd82b29a86b394ae1fc089f7b

# cognito-idp__kms__lambda__sqs: every real-verification-code flow hangs to the 60 s cap — no code is ever observable (1 cause)

> _Note: this issue was identified against the code base at commit `f04105fb2faf5f6bd82b29a86b394ae1fc089f7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ForgotPassword

## Description
Cause 1 of 1: all eight checks that need a real, delivered verification code (email OTP sign-in, forgot-password/confirm-forgot-password, resend/reissue invalidation, verify-email and verify-updated-email) make no progress and are killed by the harness at its 60 s cap.

AWS: ForgotPassword, ResendConfirmationCode, GetUserAttributeVerificationCode and the EMAIL_OTP auth flow each deliver a code through the pool's configured delivery mechanism and answer 200 with CodeDeliveryDetails; the code can then be presented to ConfirmForgotPassword / VerifyUserAttribute / RespondToAuthChallenge, which answer 200 for the current code and refuse a wrong, reused, expired or superseded code with CodeMismatchException or ExpiredCodeException (both modeled client errors on those operations). Recorded behavior only for the delivery medium used in these checks.

Emulator: none of the eight flows terminates — each reports 'Error: Check timed out after 60.0s'. The code the flow must consume never becomes observable, so neither the positive confirmation nor any of the four refusal taxonomies (wrong code, reused code, reissued-code invalidation, session expiry) can be reached.

Why: a client cannot complete a password reset, an email verification or an OTP sign-in, and cannot rely on any of the refusals that protect those flows. Calls that should settle in milliseconds hang, which is worse than an error: retries and waiters never resolve.

Fix: the cognito-idp verification-code pipeline — code generation/storage and its delivery to the configured destination, plus the code-validation branches of ConfirmForgotPassword, VerifyUserAttribute and RespondToAuthChallenge. Related open issue 20261023T114649Z-cognito-idp-cognito-user-pool-lambda-triggers-are-not-invoked.md covers trigger invocation, not code delivery, and none of these eight checks is listed there.

Traces: the eight deep-cognito-idp-1-real-code-* checks listed in trace_ids.

## Evidence
Expected: ForgotPassword returns CodeDeliveryDetails and the delivered code is accepted by ConfirmForgotPassword (200); a wrong/reused/superseded code is refused CodeMismatchException and an expired session ExpiredCodeException.
Actual: Error: Check timed out after 60.0s — for all eight flows; no code is ever delivered or consumable.

## Steps to Reproduce
1. Create a user pool whose verification code is delivered to an observable destination, plus an app client.
2. Sign a user up and call ForgotPassword (or GetUserAttributeVerificationCode, or start the EMAIL_OTP flow).
3. Try to read the delivered code and present it to ConfirmForgotPassword / VerifyUserAttribute / RespondToAuthChallenge.
4. Observe that the flow never completes.
