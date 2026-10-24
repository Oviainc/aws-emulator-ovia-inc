Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit.fast-forward-merge-refused-on-diverged-branches
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|MergeBranchesByFastForward|NotImplemented
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# Implement codecommit MergeBranchesByFastForward: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
MergeBranchesByFastForward

## Description
`MergeBranchesByFastForward` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/merge_branches_by_fast_forward.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `MergeBranchesByFastForward` call, so each one unblocks only when `MergeBranchesByFastForward` works: `codecommit.fast-forward-merge-refused-on-diverged-branches`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/merge_branches_by_fast_forward.rs`. Replaying the corpus, the emulator refused the `MergeBranchesByFastForward` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_codecommit_fast_forward_merge_refused_on_diverged_branches.py", line 104, in run
    assert code == "ManualMergeRequiredException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: fast-forward of diverged branches raised 'NotImplemented', expected ManualMergeRequiredException

## Steps to Reproduce
Boot the codecommit emulator
Call MergeBranchesByFastForward and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
