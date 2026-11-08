Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.data-plane-v2-logging-options-configured-vs-unset
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|GetV2LoggingOptions|NotImplemented
Issue-Commit: 9ad1a007c88406c445bb3f2d55ab006d23610bba

# Implement iot GetV2LoggingOptions: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `9ad1a007c88406c445bb3f2d55ab006d23610bba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetV2LoggingOptions

## Description
`GetV2LoggingOptions` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iot-awsiotservice/src/ops/get_v2_logging_options.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetV2LoggingOptions` call, so each one unblocks only when `GetV2LoggingOptions` works: `iot.data-plane-v2-logging-options-configured-vs-unset`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iot-awsiotservice/src/ops/get_v2_logging_options.rs`. Replaying the corpus, the emulator refused the `GetV2LoggingOptions` call: File "run.py", line 353, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_iot_data_plane_v2_logging_options_configured_vs_unset.py", line 29, in run
    assert code == "NotConfiguredException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected NotConfiguredException probing a possibly-unconfigured account, got 'NotImplemented'

## Steps to Reproduce
Boot the iot emulator
Call GetV2LoggingOptions and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
