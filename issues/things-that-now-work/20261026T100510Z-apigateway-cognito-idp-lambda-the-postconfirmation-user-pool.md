Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-idp__lambda.cognito-post-confirmation-trigger-invokes-lambda
Issue-Key: apigateway__cognito-idp__lambda.cognito-post-confirmation-trigger-invokes-lambda|probe#13|Timeout|resp = sqs.receive_message(
Issue-Commit: 54a3f069467f9d6acc66c8d897ec050331079988

# apigateway__cognito-idp__lambda: the PostConfirmation user-pool Lambda trigger is never invoked, so AdminConfirmSignUp produces no trigger event and the check hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `54a3f069467f9d6acc66c8d897ec050331079988`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminConfirmSignUp

## Description
One net-new root cause in the cognito-idp user-pool Lambda-trigger dispatch path.

Cause 1 of 1 — a user pool configured with LambdaConfig.PostConfirmation does not invoke that trigger when a sign-up is confirmed.

AWS: Smithy (cognito-identity-provider, services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/smithy-model/com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice.json): CreateUserPool.LambdaConfig targets LambdaConfigType, whose `PostConfirmation` member is an ArnType (@length 20..2048, pattern ^arn:...) documented as "The configuration of a post confirmation Lambda trigger in a user pool"; the shape's own doc states "Amazon Cognito invokes triggers at several possible stages of user pool operations. Triggers can modify the outcome of the operations that invoked them." The invocation itself is recorded behavior only: once AdminConfirmSignUp (or ConfirmSignUp) completes the sign-up of a user in a pool whose LambdaConfig.PostConfirmation names an invokable function, Cognito synchronously invokes that function exactly once with an event carrying triggerSource="PostConfirmation_ConfirmSignUp", userName = the confirmed user's username, userPoolId = the pool's id, and request.userAttributes containing the attributes the user signed up with (here the `email` attribute supplied to SignUp). Those four fields are the requirement; their values are taken from the request and the emulator's own state (the generated pool id, the generated username, that username's email) and are never fixed text. The trigger must be invoked under the pool ARN granted by the lambda:InvokeFunction resource policy for principal cognito-idp.amazonaws.com with SourceArn = the pool ARN.

Emulator: nothing is ever invoked. SignUp and AdminConfirmSignUp both return without error, but the function body never runs, so the queue the handler writes to stays empty and the check's SQS poll never sees a message. Replay verdict: `[mismatch] apigateway__cognito-idp__lambda.cognito-post-confirmation-trigger-invokes-lambda — Error: Check timed out after 60.0s`, hanging inside `_wait_for_message` / `_poll_until` with description "SQS trigger message" (the check's own 90 s budget exceeds the 60 s harness cap, so the hang is reported as a timeout rather than as the underlying "SQS trigger message not met" assertion).

Why: every post-sign-up side effect a client builds on this trigger silently does not happen — provisioning a profile row, adding the user to a group, writing to a downstream queue/table, sending a welcome notification. Because the confirmation call itself returns 200, the client has no way to detect that the trigger did not run: it observes a successfully confirmed user with none of the state the trigger was supposed to create, and any assertion keyed on the trigger's effect (or on triggerSource="PostConfirmation_ConfirmSignUp") never fires. A trigger that fails open is also an authorization/consistency hazard for pools that use PostConfirmation to finish enrolling the user.

Fix: the user-pool Lambda-trigger dispatch in the confirm-sign-up path of the cognito-identity-provider service (the AdminConfirmSignUp / ConfirmSignUp handlers and the shared trigger-invocation component that resolves LambdaConfig members to a Lambda invocation), plus the construction of the PostConfirmation event payload (triggerSource, userName, userPoolId, request.userAttributes). Note this is a DIFFERENT trigger and a different dispatch point from the already-open issues it resembles, none of which carry this check's replay key: 20261023T114649Z-cognito-idp-cognito-user-pool-lambda-triggers-are-not-invoked.md (PreSignUp abort, PreTokenGeneration V2_0 group override, non-invokable trigger ARN not rejected), 20261025T014422Z-apigateway-cognito-idp-lambda-the-defineauthchallenge-trigger-chain.md (DefineAuthChallenge, blocked on AdminInitiateAuth CUSTOM_AUTH), 20261025T022656Z-cognito-idp-lambda-presignup-auto-confirm-is-ignored.md (PreSignUp auto-confirm, UserMigration) and 20261025T015335Z-cognito-idp-kms-lambda-sqs-the-customemailsender-lambda.md (CustomEmailSender). If the fix for this cause depends on Lambda being able to execute a deployment package at all, that dependency is tracked by 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md — but the divergence reported here is that Cognito never issues the invocation on confirmation, which must be fixed in the Cognito trigger path regardless.

Traces: apigateway__cognito-idp__lambda.cognito-post-confirmation-trigger-invokes-lambda.

## Evidence
Cause 1: expected — after `idp.sign_up(ClientId=..., Username=<generated>, Password=..., UserAttributes=[{'Name':'email','Value':'<generated>@example.com'}])` followed by `idp.admin_confirm_sign_up(UserPoolId=<pool id>, Username=<generated>)` on a pool created with `LambdaConfig={'PostConfirmation': <fn arn>}`, the PostConfirmation function is invoked once and the event it receives satisfies triggerSource == 'PostConfirmation_ConfirmSignUp', userName == the signed-up username, userPoolId == the created pool's id, and request.userAttributes['email'] == the email supplied to SignUp (the check forwards those four fields to SQS and asserts each).
Actual — both Cognito calls return success, the function is never invoked, no message is ever produced, and the SQS poll (`receive_message` with WaitTimeSeconds=5, retried every 3 s against a 90 s budget) returns `Messages` empty on every attempt until the harness cuts the check off: `Check timed out after 60.0s` with no AWS error code or non-2xx status returned by any call. No per-run value (pool id, username, function name, queue URL) is part of the requirement; only the presence of the invocation and the four event fields derived from the request and pool state.

## Steps to Reproduce
1. Create an SQS queue and an IAM role for Lambda (trust lambda.amazonaws.com, AWSLambdaBasicExecutionRole plus an inline sqs:SendMessage allow).
2. CreateFunction (python3.12, Publish=true) whose handler reads event['triggerSource'], event['userName'], event['userPoolId'] and event['request']['userAttributes']['email'] and SendMessage's them as JSON to the queue from the QUEUE_URL environment variable; wait for Configuration.State == 'Active'.
3. CreateUserPool with DeletionProtection=INACTIVE and LambdaConfig={'PostConfirmation': <function ARN>}; note the pool's Id and Arn.
4. lambda:AddPermission on the function: StatementId='cognito-invoke', Action='lambda:InvokeFunction', Principal='cognito-idp.amazonaws.com', SourceArn=<pool ARN>.
5. CreateUserPoolClient (GenerateSecret=false, ExplicitAuthFlows including ALLOW_USER_PASSWORD_AUTH/ALLOW_ADMIN_USER_PASSWORD_AUTH).
6. SignUp a new username with an `email` attribute, then AdminConfirmSignUp that username in the pool.
7. Poll ReceiveMessage on the queue. Expected: a message within seconds whose body carries triggerSource='PostConfirmation_ConfirmSignUp' and the username/pool id/email from steps 3 and 6. Actual: the queue stays empty indefinitely and the check hangs until the 60 s harness cap.

## Resolution
The reported cause is fixed in the cognito-idp trigger path: a pool whose `LambdaConfig`
names a `PostConfirmation` function now has that function invoked synchronously when a
confirmation completes (`AdminConfirmSignUp`, `ConfirmSignUp` and `ConfirmForgotPassword`),
with the user-pool trigger event the dispatch builds from the pool and the stored user —
`triggerSource`, `userPoolId`, `userName` and `request.userAttributes` among its members
(`services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/state/lambda_triggers.rs`).

The step 7 observable of this check is still not reachable, and NOT because Cognito
withholds the invocation: the message the check polls for is one the function's own
python3.12 handler sends, so it needs Lambda to EXECUTE a deployment package. That is
tracked by `20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md`
— until it is fixed, Lambda answers `NotImplemented` for every `RequestResponse` invocation,
which this dispatch logs as a trigger that produced no verdict rather than inventing one.

