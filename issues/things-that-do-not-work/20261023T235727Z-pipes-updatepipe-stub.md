Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pipes.concurrent-update-pipe-no-corruption pipes.pipe-description-unicode-round-trip pipes.update-pipe-unknown-name-rejected-not-created pipes.update-pipe-while-creating-rejected pipes.update-pipe-while-running-preserves-desired-state
Issue-Shape-Id: com.amazonaws.pipes#Pipes
Issue-Key: pipes.concurrent-update-pipe-no-corruption|run#16|AssertionError|assert len(successes) >= 1, f"expected at least one UpdatePipe to succeed, got {results}"
Issue-Key: pipes|UpdatePipe|NotImplemented
Issue-Key: pipes.update-pipe-unknown-name-rejected-not-created|run#11|AssertionError|assert code == "NotFoundException", f"UpdatePipe on an unknown name returned {code!r}: {message!r}"
Issue-Key: pipes.update-pipe-while-creating-rejected|run#11|AssertionError|assert code == "ConflictException", f"UpdatePipe during CREATING returned {code!r}: {message!r}"
Issue-Commit: cfd0674e2ddfe499cbab1f11b70f1bc18d331a71

# Implement pipes UpdatePipe: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `cfd0674e2ddfe499cbab1f11b70f1bc18d331a71`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdatePipe

## Description
`UpdatePipe` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-pipes-pipes/src/ops/update_pipe.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `UpdatePipe` call, so each one unblocks only when `UpdatePipe` works: `pipes.concurrent-update-pipe-no-corruption`, `pipes.pipe-description-unicode-round-trip`, `pipes.update-pipe-unknown-name-rejected-not-created`, `pipes.update-pipe-while-creating-rejected`, `pipes.update-pipe-while-running-preserves-desired-state`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-pipes-pipes/src/ops/update_pipe.rs`. Replaying the corpus, the emulator refused the `UpdatePipe` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_pipes_concurrent_update_pipe_no_corruption.py", line 82, in run
    assert len(successes) >= 1, f"expected at least one UpdatePipe to succeed, got {results}"
           ^^^^^^^^^^^^^^^^^^^
AssertionError: expected at least one UpdatePipe to succeed, got [('err', 'NotImplemented', 'operation UpdatePipe is scaffolded but not implemented in this emulator'), ('err', 'NotImplemented', 'operation UpdatePipe is scaffolded but not implemented in this emulator')]

## Steps to Reproduce
Boot the pipes emulator
Call UpdatePipe and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
