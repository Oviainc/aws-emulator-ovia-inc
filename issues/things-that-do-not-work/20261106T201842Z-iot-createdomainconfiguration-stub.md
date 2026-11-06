Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.create-domain-configuration-invalid-certificate-rejected iot.domain-configuration-lifecycle iot.domain-configuration-lifecycle-transitions
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot.create-domain-configuration-invalid-certificate-rejected|run#12|AssertionError|assert err_matched == ("InvalidRequestException", _UNTRUSTED), err_matched
Issue-Key: iot|CreateDomainConfiguration|NotImplemented
Issue-Key: iot.domain-configuration-lifecycle-transitions|run#9|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code!r}: {message}"
Issue-Commit: 9ad1a007c88406c445bb3f2d55ab006d23610bba

# Implement iot CreateDomainConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `9ad1a007c88406c445bb3f2d55ab006d23610bba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomainConfiguration

## Description
`CreateDomainConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iot-awsiotservice/src/ops/create_domain_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `CreateDomainConfiguration` call, so each one unblocks only when `CreateDomainConfiguration` works: `iot.create-domain-configuration-invalid-certificate-rejected`, `iot.domain-configuration-lifecycle`, `iot.domain-configuration-lifecycle-transitions`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iot-awsiotservice/src/ops/create_domain_configuration.rs`. Replaying the corpus, the emulator refused the `CreateDomainConfiguration` call: File "run.py", line 353, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_iot_create_domain_configuration_invalid_certificate_rejected.py", line 48, in run
    assert err_matched == ("InvalidRequestException", _UNTRUSTED), err_matched
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: ('NotImplemented', 'operation CreateDomainConfiguration is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the iot emulator
Call CreateDomainConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
