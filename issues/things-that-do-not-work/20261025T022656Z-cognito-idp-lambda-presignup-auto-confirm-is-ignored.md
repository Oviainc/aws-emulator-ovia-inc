Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__lambda.pre-sign-up-auto-confirm-applies-to-sign-up-and-is-ignored-for-admin-create-user cognito-idp__lambda.user-migration-trigger-creates-the-user-and-returns-tokens-on-first-sign-in cognito-idp__lambda.user-migration-trigger-source-distinguishes-forgot-password-from-authentication
Issue-Key: cognito-idp__lambda.pre-sign-up-auto-confirm-applies-to-sign-up-and-is-ignored-for-admin-create-user|run#15|AssertionError|assert signed_up.get("UserConfirmed") is True, (
Issue-Key: cognito-identity-provider|InitiateAuth|UserNotFoundException
Issue-Key: cognito-idp__lambda|ListUsers|NotImplemented
Issue-Commit: f04105fb2faf5f6bd82b29a86b394ae1fc089f7b

# cognito-idp__lambda: PreSignUp auto-confirm is ignored, the UserMigration trigger never runs, and ListUsers is an unimplemented stub (3 causes)

> _Note: this issue was identified against the code base at commit `f04105fb2faf5f6bd82b29a86b394ae1fc089f7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SignUp

## Description
Cause 1 of 3: the PreSignUp trigger's autoConfirmUser response flag is ignored.
AWS: SignUp invokes the pool's PreSignUp trigger and honours the returned response object; with autoConfirmUser=true the user is created already confirmed and SignUp's response carries UserConfirmed=true (the same flag is deliberately NOT applied to AdminCreateUser, which is the negative half of this check). Smithy (cognito-idp) SignUp output member UserConfirmed is a required boolean.
Emulator: SignUp answers 200 with UserConfirmed=false for a pool whose PreSignUp trigger returns autoConfirmUser=true.
Why: a client that uses PreSignUp to skip confirmation sees every user land unconfirmed and cannot sign them in; the documented way to automate confirmation has no effect.
Fix: the SignUp handler's PreSignUp trigger invocation and the application of its response (autoConfirmUser / autoVerifyEmail) to the created user's state.
Traces: cognito-idp__lambda.pre-sign-up-auto-confirm-applies-to-sign-up-and-is-ignored-for-admin-create-user.

Cause 2 of 3: the UserMigration trigger is never invoked, so a first sign-in for an unknown user is refused instead of migrating them.
AWS: when a pool has a UserMigration trigger, InitiateAuth for a username that does not exist in the pool invokes the trigger with triggerSource UserMigration_Authentication; if the trigger returns userAttributes and finalUserStatus the user is created and InitiateAuth answers 200 with an AuthenticationResult. UserNotFoundException is only returned when no trigger is configured (or the trigger declines).
Emulator: InitiateAuth is refused 400 UserNotFoundException 'User does not exist.' (recorded message, an example) even though a UserMigration trigger is attached, and no tokens are issued.
Why: the entire just-in-time migration pattern is unusable — users from a legacy store can never sign in, and the client sees a plain not-found that gives no hint the trigger was skipped.
Fix: the InitiateAuth handler's unknown-user path in cognito-idp: invoke the UserMigration trigger and create the user from its response before refusing.
Traces: cognito-idp__lambda.user-migration-trigger-creates-the-user-and-returns-tokens-on-first-sign-in.

Cause 3 of 3: ListUsers answers NotImplemented.
AWS: Smithy (cognito-idp) ListUsers answers 200 with Users (paginated, Filter/AttributesToGet supported); its modeled refusals are InvalidParameterException, NotAuthorizedException, ResourceNotFoundException and TooManyRequestsException.
Emulator: the call is refused with code 'NotImplemented' and message 'operation ListUsers is scaffolded but not implemented in this emulator' (recorded message, an example).
Why: NotImplemented is not in the operation's error set, so a client cannot enumerate pool users or verify the effect of a migration/forgot-password trigger at all, and its error handling for ListUsers is never exercised.
Fix: implement the ListUsers handler in the cognito-idp service (user store enumeration + Filter and pagination).
Traces: cognito-idp__lambda.user-migration-trigger-source-distinguishes-forgot-password-from-authentication.

## Evidence
Cause 1 — Expected: SignUp returns UserConfirmed=true when PreSignUp sets autoConfirmUser=true. Actual: AssertionError: autoConfirmUser=true must make SignUp report UserConfirmed=true, got False.
Cause 2 — Expected: InitiateAuth for an unknown user with a UserMigration trigger returns tokens. Actual: botocore.errorfactory.UserNotFoundException: ... when calling the InitiateAuth operation: User does not exist.
Cause 3 — Expected: ListUsers returns 200 with the pool's users. Actual: ClientError (NotImplemented): operation ListUsers is scaffolded but not implemented in this emulator.

## Steps to Reproduce
Cause 1: create a pool with a PreSignUp trigger returning {'response': {'autoConfirmUser': True}}; call SignUp and read UserConfirmed.
Cause 2: create a pool with a UserMigration trigger that returns userAttributes + finalUserStatus; call InitiateAuth (USER_PASSWORD_AUTH) for a username that was never created.
Cause 3: call ListUsers on any pool.
