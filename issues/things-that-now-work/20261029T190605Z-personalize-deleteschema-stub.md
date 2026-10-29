Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: personalize.schema-delete-blocked-while-dataset-references-it
Issue-Shape-Id: com.amazonaws.personalize#AmazonPersonalize
Issue-Key: personalize.schema-delete-blocked-while-dataset-references-it|run#13|AssertionError|assert code == "ResourceInUseException", f"expected ResourceInUseException, got {code}: {message!r}"
Issue-Commit: 69bb2c4e0c47afe7ac06c67651974f0cf752730a

# Implement personalize DeleteSchema: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `69bb2c4e0c47afe7ac06c67651974f0cf752730a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSchema

## Description
`DeleteSchema` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-personalize-amazonpersonalize/src/ops/delete_schema.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteSchema` call, so each one unblocks only when `DeleteSchema` works: `personalize.schema-delete-blocked-while-dataset-references-it`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-personalize-amazonpersonalize/src/ops/delete_schema.rs`. Replaying the corpus, the emulator refused the `DeleteSchema` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_personalize_schema_delete_blocked_while_dataset_references_it.py", line 102, in run
    assert code == "ResourceInUseException", f"expected ResourceInUseException, got {code}: {message!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ResourceInUseException, got NotImplemented: 'operation DeleteSchema is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the personalize emulator
Call DeleteSchema and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
