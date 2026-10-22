Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elementalinference.get-fixture-not-found
Issue-Shape-Id: com.amazonaws.elementalinference#ElementalInference
Issue-Key: elementalinference.get-fixture-not-found|run#2|AssertionError|assert err.get("Code") == "ValidationException", err
Issue-Commit: 668d747e62a7a896ef2863c29b158e984b097a40

# Implement elementalinference GetFixture: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `668d747e62a7a896ef2863c29b158e984b097a40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFixture

## Description
`GetFixture` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-elementalinference-elementalinference/src/ops/get_fixture.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetFixture` call, so each one unblocks only when `GetFixture` works: `elementalinference.get-fixture-not-found`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-elementalinference-elementalinference/src/ops/get_fixture.rs`. Replaying the corpus, the emulator refused the `GetFixture` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_elementalinference_get_fixture_not_found.py", line 34, in run
    assert err.get("Code") == "ValidationException", err
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: {'Message': 'operation GetFixture is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the elementalinference emulator
Call GetFixture and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
