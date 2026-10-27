Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__lambda.user-migration-trigger-source-distinguishes-forgot-password-from-authentication
Issue-Key: cognito-idp__lambda.user-migration-trigger-source-distinguishes-forgot-password-from-authentication|_retry_on_lambda_warmup#15|Timeout|time.sleep(3.0)
Issue-Commit: 9a356a0438a215c6e535efb212b208d427610e62

# cognito-idp__lambda: the UserMigration trigger is never driven by ForgotPassword, so triggerSource never distinguishes UserMigration_ForgotPassword from UserMigration_Authentication (1 cause)

> _Note: this issue was identified against the code base at commit `9a356a0438a215c6e535efb212b208d427610e62`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ForgotPassword

## Description
One net-new root cause, grounded in one failing behavioral check (Cause 1 of 1).
AWS: recorded behavior only for the trigger payload (the Cognito Identity Provider model describes the LambdaConfig member UserMigration but not the event envelope). AWS documents the migrate-user Lambda trigger as being invoked with triggerSource `UserMigration_Authentication` when an unknown user signs in (InitiateAuth/AdminInitiateAuth with USER_PASSWORD_AUTH) and with triggerSource `UserMigration_ForgotPassword` when an unknown user starts a password reset (ForgotPassword), with the trigger's returned userAttributes/finalUserStatus/messageAction creating the user in the pool before the originating call completes.
Emulator: the check makes no progress and is cut off by the suite's per-check deadline — `Error: Check timed out after 60.0s` — with neither trigger invocation observable. Hypothesis: the user pool's UserMigration LambdaConfig is never dispatched, so neither the authentication path nor the forgot-password path invokes the Lambda, the unknown user is never created, and the call the check waits on never completes. The open issues 20261025T022656Z-cognito-idp-lambda-presignup-auto-confirm-is-ignored.md (which owns the sibling check `user-migration-trigger-creates-the-user-and-returns-tokens-on-first-sign-in`) and 20261024T114943Z-cognito-idp-lambda-the-pool-s-jwks-document.md cover neighbouring causes but do not carry this replay key: what is new here is that the ForgotPassword path must drive the SAME trigger with a DIFFERENT triggerSource.
Why: a client migrating users from a legacy directory relies on the trigger firing on both entry points and on triggerSource to tell the two apart (an authentication migration validates the submitted password; a forgot-password migration must not). With no invocation at all, migration never happens, an unknown user's ForgotPassword/InitiateAuth never succeeds, and code branching on triggerSource is dead.
Fix: the Cognito user-pool trigger dispatcher — ForgotPassword and InitiateAuth/AdminInitiateAuth handlers must invoke the pool's UserMigration Lambda for an unknown username, set triggerSource to UserMigration_ForgotPassword and UserMigration_Authentication respectively, and apply the returned userAttributes/finalUserStatus to create the user before answering.
Traces: cognito-idp__lambda.user-migration-trigger-source-distinguishes-forgot-password-from-authentication.

## Evidence
Report detail for cognito-idp__lambda.user-migration-trigger-source-distinguishes-forgot-password-from-authentication: `Error: Check timed out after 60.0s`.
Expected: ForgotPassword for a username that does not exist in the pool invokes the pool's UserMigration Lambda with triggerSource `UserMigration_ForgotPassword` (and the sign-in path invokes the same Lambda with `UserMigration_Authentication`), the user is created from the trigger's returned attributes, and the originating call completes.
Actual: no invocation is observable on either path and the originating call never completes within the window, so the two triggerSource values are never produced.

## Steps to Reproduce
CreateUserPool with LambdaConfig.UserMigration pointing at a Lambda that records the event it receives (e.g. forwards triggerSource to SQS/CloudWatch Logs) and returns userAttributes plus finalUserStatus CONFIRMED; add the lambda:InvokeFunction permission for cognito-idp.amazonaws.com; create an app client. Then (a) `aws cognito-idp forgot-password --client-id <id> --username <not-in-pool>` and (b) `aws cognito-idp initiate-auth --auth-flow USER_PASSWORD_AUTH --client-id <id> --auth-parameters USERNAME=<other-not-in-pool>,PASSWORD=<pw>`. Observe no UserMigration invocation is recorded for either, no user is created, and neither triggerSource value (UserMigration_ForgotPassword / UserMigration_Authentication) is ever seen. Replay: cognito-idp__lambda.user-migration-trigger-source-distinguishes-forgot-password-from-authentication.
