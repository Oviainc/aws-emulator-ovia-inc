Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-idp__lambda.cli-lambda-cognito-trigger-setup-and-test cognito-idp__lambda.custom-auth-challenge-triad-issues-tokens-only-after-the-secret-answer cognito-idp__lambda.custom-auth-wrong-answer-rejected-and-no-token-is-issued cognito-idp__lambda.pre-authentication-trigger-denial-rejected-and-the-allowed-user-still-signs-in cognito-idp__lambda.pre-sign-up-auto-confirm-applies-to-sign-up-and-is-ignored-for-admin-create-user cognito-idp__lambda.user-migration-trigger-creates-the-user-and-returns-tokens-on-first-sign-in cognito-idp__lambda.user-migration-trigger-source-distinguishes-forgot-password-from-authentication cognito-idp__lambda.xsvc-cognito-idp-3-postconfirmation-lambda-raises-confirmsignup-fails-but-user-stays-confirmed cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-lambda-sleeps-past-five-seconds-signin-fails cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-v2-triggersource-distinguishes-refresh-from-authentication
Issue-Key: apigateway__cognito-idp__lambda.cli-lambda-cognito-trigger-setup-and-test|_run_shell#24|AssertionError|raise AssertionError(
Issue-Key: cognito-identity-provider|UpdateUserPool|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement cognito-identity-provider UpdateUserPool: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateUserPool

## Description
`UpdateUserPool` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/update_user_pool.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

10 recorded unit(s) fail on their `UpdateUserPool` call, so each one unblocks only when `UpdateUserPool` works: `apigateway__cognito-idp__lambda.cli-lambda-cognito-trigger-setup-and-test`, `cognito-idp__lambda.custom-auth-challenge-triad-issues-tokens-only-after-the-secret-answer`, `cognito-idp__lambda.custom-auth-wrong-answer-rejected-and-no-token-is-issued`, `cognito-idp__lambda.pre-authentication-trigger-denial-rejected-and-the-allowed-user-still-signs-in`, `cognito-idp__lambda.pre-sign-up-auto-confirm-applies-to-sign-up-and-is-ignored-for-admin-create-user`, `cognito-idp__lambda.user-migration-trigger-creates-the-user-and-returns-tokens-on-first-sign-in`, `cognito-idp__lambda.user-migration-trigger-source-distinguishes-forgot-password-from-authentication`, `cognito-idp__lambda.xsvc-cognito-idp-3-postconfirmation-lambda-raises-confirmsignup-fails-but-user-stays-confirmed`, `cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-lambda-sleeps-past-five-seconds-signin-fails`, `cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-v2-triggersource-distinguishes-refresh-from-authentication`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/update_user_pool.rs`. Replaying the corpus, the emulator refused the `UpdateUserPool` call: File "check_apigateway__cognito_idp__lambda_cli_lambda_cognito_trigger_setup_and_test.py", line 159, in _run_aws
    proc = _run_shell(ctx, ["aws", "--output", "json", *args], timeout_s=timeout_s, check=check)
  File "check_apigateway__cognito_idp__lambda_cli_lambda_cognito_trigger_setup_and_test.py", line 151, in _run_shell
    raise AssertionError(
    ...<2 lines>...
    )
AssertionError: Command failed (rc=254):
  cmd: ['aws', '--output', 'json', 'cognito-idp', 'update-user-pool', '--user-pool-id', 'us-west-2_39e6f417e', '--lambda-config', 'PreSignUp=arn:aws:lambda:us-west-2:000000000000:function:sim2real-trig-fn-dde593fec09a']
  stderr: 
aws: [ERROR]: An error occurred (NotImplemented) when calling the UpdateUserPool operation: operation UpdateUserPool is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cognito-identity-provider emulator
Call UpdateUserPool and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
