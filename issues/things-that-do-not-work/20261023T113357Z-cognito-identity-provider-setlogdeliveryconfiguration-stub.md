Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__logs.obs-cognito-idp-1-log-delivery-config cognito-idp__logs.obs-cognito-idp-1-security-audit-telemetry
Issue-Key: cognito-idp__logs.obs-cognito-idp-1-log-delivery-config|run#12|AssertionError|assert code == "InvalidParameterException", (
Issue-Key: cognito-identity-provider|SetLogDeliveryConfiguration|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement cognito-identity-provider SetLogDeliveryConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetLogDeliveryConfiguration

## Description
`SetLogDeliveryConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/set_log_delivery_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `SetLogDeliveryConfiguration` call, so each one unblocks only when `SetLogDeliveryConfiguration` works: `logs__cognito-idp.obs-cognito-idp-1-log-delivery-config`, `logs__cognito-idp.obs-cognito-idp-1-security-audit-telemetry`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/set_log_delivery_configuration.rs`. Replaying the corpus, the emulator refused the `SetLogDeliveryConfiguration` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_logs__cognito_idp_obs_cognito_idp_1_log_delivery_config.py", line 61, in run
    assert code == "InvalidParameterException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: a never-created log group as destination: expected InvalidParameterException, got 'NotImplemented' ('operation SetLogDeliveryConfiguration is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the cognito-identity-provider emulator
Call SetLogDeliveryConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
