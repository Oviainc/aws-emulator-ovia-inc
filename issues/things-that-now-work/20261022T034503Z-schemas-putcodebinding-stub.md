Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: schemas.put-code-binding-unsupported-language-rejected
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas.put-code-binding-unsupported-language-rejected|run#9|AssertionError|assert err["Code"] == "BadRequestException", f"unexpected error: {err!r}"
Issue-Commit: 37e4b54b7f2f231778aa0090f22742466dbbad1b

# Implement schemas PutCodeBinding: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `37e4b54b7f2f231778aa0090f22742466dbbad1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutCodeBinding

## Description
`PutCodeBinding` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-schemas-schemas/src/ops/put_code_binding.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `PutCodeBinding` call, so each one unblocks only when `PutCodeBinding` works: `schemas.put-code-binding-unsupported-language-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-schemas-schemas/src/ops/put_code_binding.rs`. Replaying the corpus, the emulator refused the `PutCodeBinding` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_schemas_put_code_binding_unsupported_language_rejected.py", line 38, in run
    assert err["Code"] == "BadRequestException", f"unexpected error: {err!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: unexpected error: {'Message': 'operation PutCodeBinding is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the schemas emulator
Call PutCodeBinding and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
