Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling.delete-scheduled-action-that-does-not-exist-rejected
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-autoscaling.delete-scheduled-action-that-does-not-exist-rejected|run#13|AssertionError|assert error.get('Code') == 'ObjectNotFoundException', (
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Implement application-autoscaling DeleteScheduledAction: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteScheduledAction

## Description
`DeleteScheduledAction` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/delete_scheduled_action.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteScheduledAction` call, so each one unblocks only when `DeleteScheduledAction` works: `application-autoscaling.delete-scheduled-action-that-does-not-exist-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/delete_scheduled_action.rs`. Replaying the corpus, the emulator refused the `DeleteScheduledAction` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_application_autoscaling_delete_scheduled_action_that_does_not_exist_rejected.py", line 111, in run
    assert error.get('Code') == 'ObjectNotFoundException', (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: deleting a missing scheduled action: expected ObjectNotFoundException, got {'Message': 'operation DeleteScheduledAction is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the application-autoscaling emulator
Call DeleteScheduledAction and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
