Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.stack-instance-and-operation-not-found-errors
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.stack-instance-and-operation-not-found-errors|_expect_error#5|AssertionError|assert got_code == code, f"expected {code}, got {got_code!r} ({got_msg!r})"
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# Implement cloudformation ListStackInstances: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `461d317fc71f56ed8ab822edc66618d36d651bfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListStackInstances

## Description
`ListStackInstances` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/list_stack_instances.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListStackInstances` call, so each one unblocks only when `ListStackInstances` works: `cloudformation.stack-instance-and-operation-not-found-errors`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/list_stack_instances.rs`. Replaying the corpus, the emulator refused the `ListStackInstances` call: File "check_cloudformation_stack_instance_and_operation_not_found_errors.py", line 75, in run
    _expect_error(
    ~~~~~~~~~~~~~^
        lambda: cf.list_stack_instances(StackSetName=missing_stackset),
        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
        "StackSetNotFoundException",
        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^
        f"StackSet {missing_stackset} not found",
        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    )
    ^
  File "check_cloudformation_stack_instance_and_operation_not_found_errors.py", line 29, in _expect_error
    assert got_code == code, f"expected {code}, got {got_code!r} ({got_msg!r})"
           ^^^^^^^^^^^^^^^^
AssertionError: expected StackSetNotFoundException, got 'NotImplemented' ('operation ListStackInstances is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the cloudformation emulator
Call ListStackInstances and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
