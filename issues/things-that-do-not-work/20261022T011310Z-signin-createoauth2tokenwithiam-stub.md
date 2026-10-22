Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: signin.create-oauth2-token-with-iam-rotates-access-token signin.create-oauth2-token-with-iam-success signin.introspect-oauth2-token-with-iam-hint-optional signin.introspect-oauth2-token-with-iam-resource-claim-round-trip-preserved
Issue-Shape-Id: com.amazonaws.signin#Signin
Issue-Key: signin|CreateOAuth2TokenWithIAM|NotImplemented
Issue-Commit: 872e0ddf6cb614931de58689afe8393c02abd052

# Implement signin CreateOAuth2TokenWithIAM: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `872e0ddf6cb614931de58689afe8393c02abd052`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateOAuth2TokenWithIAM

## Description
`CreateOAuth2TokenWithIAM` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-signin-signin/src/ops/create_o_auth2_token_with_iam.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `CreateOAuth2TokenWithIAM` call, so each one unblocks only when `CreateOAuth2TokenWithIAM` works: `signin.create-oauth2-token-with-iam-rotates-access-token`, `signin.create-oauth2-token-with-iam-success`, `signin.introspect-oauth2-token-with-iam-hint-optional`, `signin.introspect-oauth2-token-with-iam-resource-claim-round-trip-preserved`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-signin-signin/src/ops/create_o_auth2_token_with_iam.rs`. Replaying the corpus, the emulator refused the `CreateOAuth2TokenWithIAM` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateOAuth2TokenWithIAM operation: operation CreateOAuth2TokenWithIAM is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the signin emulator
Call CreateOAuth2TokenWithIAM and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
