Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatchomni.omni-dashboard-body-must-be-a-panels-document
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni.omni-dashboard-body-must-be-a-panels-document|run#13|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}: {message!r}"
Issue-Commit: eaa7cb6623904c9173b712c8794ba33cc0f5c9ec

# Implement cloudwatchomni CreateOmniDashboard: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `eaa7cb6623904c9173b712c8794ba33cc0f5c9ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateOmniDashboard

## Description
`CreateOmniDashboard` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/create_omni_dashboard.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateOmniDashboard` call, so each one unblocks only when `CreateOmniDashboard` works: `cloudwatchomni.omni-dashboard-body-must-be-a-panels-document`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/create_omni_dashboard.rs`. Replaying the corpus, the emulator refused the `CreateOmniDashboard` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_cloudwatchomni_omni_dashboard_body_must_be_a_panels_document.py", line 70, in run
    assert code == "ValidationException", f"expected ValidationException, got {code}: {message!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationException, got NotImplemented: 'operation CreateOmniDashboard is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the cloudwatchomni emulator
Call CreateOmniDashboard and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
