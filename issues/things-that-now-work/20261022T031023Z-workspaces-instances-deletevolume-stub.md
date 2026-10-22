Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: workspaces-instances.delete-volume-not-found
Issue-Shape-Id: com.amazonaws.workspacesinstances#EUCMIFrontendAPIService
Issue-Key: workspaces-instances.delete-volume-not-found|run#4|AssertionError|assert code == "ResourceNotFoundException", (code, message)
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# Implement workspaces-instances DeleteVolume: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteVolume

## Description
`DeleteVolume` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/src/ops/delete_volume.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteVolume` call, so each one unblocks only when `DeleteVolume` works: `workspaces-instances.delete-volume-not-found`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/src/ops/delete_volume.rs`. Replaying the corpus, the emulator refused the `DeleteVolume` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_workspaces_instances_delete_volume_not_found.py", line 18, in run
    assert code == "ResourceNotFoundException", (code, message)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: ('NotImplemented', 'operation DeleteVolume is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the workspaces-instances emulator
Call DeleteVolume and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
