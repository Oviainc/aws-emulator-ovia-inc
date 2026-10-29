Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.concurrent-glossary-delete-conflict-detected
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.concurrent-glossary-delete-conflict-detected|run#10|AssertionError|assert update_outcome["error"] in (None, conflict, not_found), outcomes
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# Implement glue UpdateGlossary: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateGlossary

## Description
`UpdateGlossary` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-glue-awsglue/src/ops/update_glossary.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateGlossary` call, so each one unblocks only when `UpdateGlossary` works: `glue.concurrent-glossary-delete-conflict-detected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-glue-awsglue/src/ops/update_glossary.rs`. Replaying the corpus, the emulator refused the `UpdateGlossary` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_glue_concurrent_glossary_delete_conflict_detected.py", line 39, in run
    assert update_outcome["error"] in (None, conflict, not_found), outcomes
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: [{'result': None, 'error': ('NotImplemented', 'operation UpdateGlossary is scaffolded but not implemented in this emulator')}, {'result': {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', 'date': 'Thu, 29 Oct 2026 18:28:09 GMT'}, 'RetryAttempts': 0}}, 'error': None}]

## Steps to Reproduce
Boot the glue emulator
Call UpdateGlossary and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
