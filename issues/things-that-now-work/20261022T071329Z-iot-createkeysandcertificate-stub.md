Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotdeviceadvisor.concurrent-start-suite-run-only-one-wins iotdeviceadvisor.get-suite-run-report-not-ready-before-completion iotdeviceadvisor.list-suite-runs-reflects-started-run iotdeviceadvisor.start-suite-run-conflict-when-already-running-rejected iotdeviceadvisor.start-suite-run-creates-pending-or-running-run iotdeviceadvisor.stop-suite-run-initiates-stopping-transition iotdeviceadvisor.stop-suite-run-repeat-after-completion-rejected iotdeviceadvisor.suite-run-without-device-fails-on-connect-timeout
Issue-Shape-Id: com.amazonaws.iotdeviceadvisor#IotSenateService
Issue-Key: iot|CreateKeysAndCertificate|NotImplemented
Issue-Commit: d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea

# Implement iot CreateKeysAndCertificate: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateKeysAndCertificate

## Description
`CreateKeysAndCertificate` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iot-awsiotservice/src/ops/create_keys_and_certificate.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

8 recorded unit(s) fail on their `CreateKeysAndCertificate` call, so each one unblocks only when `CreateKeysAndCertificate` works: `iotdeviceadvisor.concurrent-start-suite-run-only-one-wins`, `iotdeviceadvisor.get-suite-run-report-not-ready-before-completion`, `iotdeviceadvisor.list-suite-runs-reflects-started-run`, `iotdeviceadvisor.start-suite-run-conflict-when-already-running-rejected`, `iotdeviceadvisor.start-suite-run-creates-pending-or-running-run`, `iotdeviceadvisor.stop-suite-run-initiates-stopping-transition`, `iotdeviceadvisor.stop-suite-run-repeat-after-completion-rejected`, `iotdeviceadvisor.suite-run-without-device-fails-on-connect-timeout`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iot-awsiotservice/src/ops/create_keys_and_certificate.rs`. Replaying the corpus, the emulator refused the `CreateKeysAndCertificate` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateKeysAndCertificate operation: operation CreateKeysAndCertificate is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the iot emulator
Call CreateKeysAndCertificate and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
