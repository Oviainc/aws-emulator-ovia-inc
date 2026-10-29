Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: datazone.glossary-delete-with-terms-rejected
Issue-Shape-Id: com.amazonaws.datazone#DataZone
Issue-Key: datazone.glossary-delete-with-terms-rejected|run#8|AssertionError|assert code == "ConflictException", (
Issue-Commit: 2f933b71caa2a46f81d276f12cbb60da2946d084

# Implement datazone DeleteGlossary: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2f933b71caa2a46f81d276f12cbb60da2946d084`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteGlossary

## Description
`DeleteGlossary` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-datazone-datazone/src/ops/delete_glossary.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteGlossary` call, so each one unblocks only when `DeleteGlossary` works: `datazone.glossary-delete-with-terms-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-datazone-datazone/src/ops/delete_glossary.rs`. Replaying the corpus, the emulator refused the `DeleteGlossary` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_datazone_glossary_delete_with_terms_rejected.py", line 39, in run
    assert code == "ConflictException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ConflictException deleting a disabled glossary that still has a term, got 'NotImplemented' ('operation DeleteGlossary is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the datazone emulator
Call DeleteGlossary and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
