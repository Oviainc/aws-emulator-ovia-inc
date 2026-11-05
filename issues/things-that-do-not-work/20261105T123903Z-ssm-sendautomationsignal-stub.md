Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.send-automation-signal-invalid-state-rejected ssm.send-automation-signal-nonexistent-step-rejected
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.send-automation-signal-invalid-state-rejected|run#13|AssertionError|assert early_resume == (
Issue-Key: ssm|SendAutomationSignal|NotImplemented
Issue-Commit: e924affd30ac1a5870106526f18d5e4efb90f778

# Implement ssm SendAutomationSignal: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `e924affd30ac1a5870106526f18d5e4efb90f778`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendAutomationSignal

## Description
`SendAutomationSignal` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ssm-amazonssm/src/ops/send_automation_signal.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `SendAutomationSignal` call, so each one unblocks only when `SendAutomationSignal` works: `ssm.send-automation-signal-invalid-state-rejected`, `ssm.send-automation-signal-nonexistent-step-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ssm-amazonssm/src/ops/send_automation_signal.rs`. Replaying the corpus, the emulator refused the `SendAutomationSignal` call: File "run.py", line 348, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_ssm_send_automation_signal_invalid_state_rejected.py", line 72, in run
    assert early_resume == (
           ^^^^^^^^^^^^^^^^^
AssertionError: ('NotImplemented', 'operation SendAutomationSignal is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the ssm emulator
Call SendAutomationSignal and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
