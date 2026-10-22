Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeguruprofiler.get-recommendations-start-after-end-rejected
Issue-Shape-Id: com.amazonaws.codeguruprofiler#CodeGuruProfiler
Issue-Key: codeguruprofiler.get-recommendations-start-after-end-rejected|run#10|AssertionError|assert code == "ValidationException", (code, message)
Issue-Commit: 699f6ed7a7b73d87a9db3c950b6f69a120aa392c

# Implement codeguruprofiler GetRecommendations: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `699f6ed7a7b73d87a9db3c950b6f69a120aa392c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRecommendations

## Description
`GetRecommendations` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/get_recommendations.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetRecommendations` call, so each one unblocks only when `GetRecommendations` works: `codeguruprofiler.get-recommendations-start-after-end-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/get_recommendations.rs`. Replaying the corpus, the emulator refused the `GetRecommendations` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_codeguruprofiler_get_recommendations_start_after_end_rejected.py", line 40, in run
    assert code == "ValidationException", (code, message)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: ('NotImplemented', 'operation GetRecommendations is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the codeguruprofiler emulator
Call GetRecommendations and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
