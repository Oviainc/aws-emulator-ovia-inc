Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: quicksight.role-membership-group-lifecycle
Issue-Shape-Id: com.amazonaws.quicksight#QuickSight_20180401
Issue-Key: quicksight.role-membership-group-lifecycle|run#10|AssertionError|assert list_err["Code"] == "AccessDeniedException", (
Issue-Commit: c0b127c099d8543a6fa74c4b10d6d49cf92e859e

# Implement quicksight ListRoleMemberships: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c0b127c099d8543a6fa74c4b10d6d49cf92e859e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRoleMemberships

## Description
`ListRoleMemberships` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-quicksight-quicksight-20180401/src/ops/list_role_memberships.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListRoleMemberships` call, so each one unblocks only when `ListRoleMemberships` works: `quicksight.role-membership-group-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-quicksight-quicksight-20180401/src/ops/list_role_memberships.rs`. Replaying the corpus, the emulator refused the `ListRoleMemberships` call: File "run.py", line 353, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_quicksight_role_membership_group_lifecycle.py", line 32, in run
    assert list_err["Code"] == "AccessDeniedException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ListRoleMemberships to be denied for an IAM_ONLY identity store, got {'Message': 'operation ListRoleMemberships is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the quicksight emulator
Call ListRoleMemberships and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
