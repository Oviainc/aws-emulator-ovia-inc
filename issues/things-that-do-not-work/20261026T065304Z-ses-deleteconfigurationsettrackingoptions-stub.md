Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.tracking-options-delete-on-unset-set-fails
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses.tracking-options-delete-on-unset-set-fails|run#6|AssertionError|assert err == (
Issue-Commit: 3d58e892724a4dea7104d79d915d87cd1968e39f

# Implement ses DeleteConfigurationSetTrackingOptions: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `3d58e892724a4dea7104d79d915d87cd1968e39f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteConfigurationSetTrackingOptions

## Description
`DeleteConfigurationSetTrackingOptions` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ses-simpleemailservice/src/ops/delete_configuration_set_tracking_options.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteConfigurationSetTrackingOptions` call, so each one unblocks only when `DeleteConfigurationSetTrackingOptions` works: `ses.tracking-options-delete-on-unset-set-fails`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ses-simpleemailservice/src/ops/delete_configuration_set_tracking_options.rs`. Replaying the corpus, the emulator refused the `DeleteConfigurationSetTrackingOptions` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_ses_tracking_options_delete_on_unset_set_fails.py", line 41, in run
    assert err == (
           ^^^^^^^^
    ...<2 lines>...
    ), f"expected TrackingOptionsDoesNotExist when no tracking options exist, got {err!r}"
    ^
AssertionError: expected TrackingOptionsDoesNotExist when no tracking options exist, got ('NotImplemented', 'operation DeleteConfigurationSetTrackingOptions is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the ses emulator
Call DeleteConfigurationSetTrackingOptions and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
