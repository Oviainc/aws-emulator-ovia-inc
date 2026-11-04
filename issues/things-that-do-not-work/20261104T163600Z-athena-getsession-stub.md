Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.session-ops-reject-unknown-session
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena.session-ops-reject-unknown-session|_assert_all_reject#6|AssertionError|assert not deviations, f"deviations from the expected (code, message): {deviations}"
Issue-Commit: 1b83187822ca52f98e00143d7b0b9c4d0812f404

# Implement athena GetSession: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `1b83187822ca52f98e00143d7b0b9c4d0812f404`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSession

## Description
`GetSession` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-athena-amazonathena/src/ops/get_session.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetSession` call, so each one unblocks only when `GetSession` works: `athena.session-ops-reject-unknown-session`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-athena-amazonathena/src/ops/get_session.rs`. Replaying the corpus, the emulator refused the `GetSession` call: File "check_athena_session_ops_reject_unknown_session.py", line 46, in run
    _assert_all_reject(
  File "check_athena_session_ops_reject_unknown_session.py", line 33, in _assert_all_reject
    assert not deviations, f"deviations from the expected (code, message): {deviations}"
           ^^^^^^^^^^^^^^
AssertionError: deviations from the expected (code, message): {'GetSession': ('NotImplemented', 'operation GetSession is scaffolded but not implemented in this emulator'), 'GetSessionStatus': ('NotImplemented', 'operation GetSessionStatus is scaffolded but not implemented in this emulator'), 'ListExecutors': ('NotImplemented', 'operation ListExecutors is scaffolded but not implemented in this emulator'), 'ListCalculationExecutions': ('NotImplemented', 'operation ListCalculationExecutions is scaffolded but not implemented in this emulator'), 'TerminateSession': ('NotImplemented', 'operation TerminateSession is scaffolded but not implemented in this emulator'), 'CreatePresignedNotebookUrl': ('NotImplemented', 'operation CreatePresignedNotebookUrl is scaffolded but not implemented in this emulator'), 'StartCalculationExecution': ('NotImplemented', 'operation StartCalculationExecution is scaffolded but not implemented in this emulator'), 'ListSessions': ('NotImplemented', 'operation ListSessions is scaffolded but not implemented in this emulator'), 'StartSession': ('NotImplemented', 'operation StartSession is scaffolded but not implemented in this emulator')}

## Steps to Reproduce
Boot the athena emulator
Call GetSession and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
