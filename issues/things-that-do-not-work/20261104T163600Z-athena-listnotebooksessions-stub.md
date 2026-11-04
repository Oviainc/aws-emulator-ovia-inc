Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.notebook-sessions-list-unknown-notebook-not-found
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena.notebook-sessions-list-unknown-notebook-not-found|run#5|AssertionError|assert got == expected, f"expected an unknown NotebookId to raise {expected}, got {got}"
Issue-Commit: 1b83187822ca52f98e00143d7b0b9c4d0812f404

# Implement athena ListNotebookSessions: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `1b83187822ca52f98e00143d7b0b9c4d0812f404`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListNotebookSessions

## Description
`ListNotebookSessions` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-athena-amazonathena/src/ops/list_notebook_sessions.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListNotebookSessions` call, so each one unblocks only when `ListNotebookSessions` works: `athena.notebook-sessions-list-unknown-notebook-not-found`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-athena-amazonathena/src/ops/list_notebook_sessions.rs`. Replaying the corpus, the emulator refused the `ListNotebookSessions` call: File "run.py", line 348, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_athena_notebook_sessions_list_unknown_notebook_not_found.py", line 28, in run
    assert got == expected, f"expected an unknown NotebookId to raise {expected}, got {got}"
           ^^^^^^^^^^^^^^^
AssertionError: expected an unknown NotebookId to raise ('InvalidRequestException', 'Notebook 3b452d38-d2ed-4a59-a9c0-4892c61d559b was not found'), got ('NotImplemented', 'operation ListNotebookSessions is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the athena emulator
Call ListNotebookSessions and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
