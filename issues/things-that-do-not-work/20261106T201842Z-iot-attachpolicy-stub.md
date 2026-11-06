Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.attach-policy-nonexistent-target-rejected
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot.attach-policy-nonexistent-target-rejected|run#9|AssertionError|assert err == ("ResourceNotFoundException", "The certificate given in the principal does not exist."), (
Issue-Commit: 9ad1a007c88406c445bb3f2d55ab006d23610bba

# Implement iot AttachPolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `9ad1a007c88406c445bb3f2d55ab006d23610bba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AttachPolicy

## Description
`AttachPolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iot-awsiotservice/src/ops/attach_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `AttachPolicy` call, so each one unblocks only when `AttachPolicy` works: `iot.attach-policy-nonexistent-target-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iot-awsiotservice/src/ops/attach_policy.rs`. Replaying the corpus, the emulator refused the `AttachPolicy` call: File "run.py", line 353, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_iot_attach_policy_nonexistent_target_rejected.py", line 39, in run
    assert err == ("ResourceNotFoundException", "The certificate given in the principal does not exist."), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ResourceNotFoundException attaching to a nonexistent target, got ('NotImplemented', 'operation AttachPolicy is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the iot emulator
Call AttachPolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
