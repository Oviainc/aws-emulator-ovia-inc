Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sqs.cdk-refactor-refuses-a-combined-rename-and-property-change
Issue-Key: cloudformation__sqs.cdk-refactor-refuses-a-combined-rename-and-property-change|run#36|AssertionError|assert rc == 0 and "Stack refactor complete" in out, (
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# Implement cloudformation ExecuteStackRefactor: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStackRefactor

## Description
`ExecuteStackRefactor` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/execute_stack_refactor.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ExecuteStackRefactor` call, so each one unblocks only when `ExecuteStackRefactor` works: `cloudformation__sqs.cdk-refactor-refuses-a-combined-rename-and-property-change`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/execute_stack_refactor.rs`. Replaying the corpus, the emulator refused the `ExecuteStackRefactor` call: File "run.py", line 354, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_cloudformation__sqs_cdk_refactor_refuses_a_combined_rename_and_property_change.py", line 135, in run
    assert rc == 0 and "Stack refactor complete" in out, (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: a PURE rename (no property change) was refused: rc=0; output tail: '✨  Synthesis time: 2.04s\naws://000000000000/us-west-2\nThe following resources were moved or renamed:\n┌─────────────────┬─────────────────────────────────────────────┬────────────────────────────────────────────────────┐\n│ Resource Type   │ Old Construct Path                          │ New Construct Path                                 │\n├─────────────────┼─────────────────────────────────────────────┼────────────────────────────────────────────────────┤\n│ AWS::SQS::Queue │ sim2real-cdkrfn-2979a39fd7fd/Queue/Resource │ sim2real-cdkrfn-2979a39fd7fd/QueueRenamed/Resource │\n└─────────────────┴─────────────────────────────────────────────┴────────────────────────────────────────────────────┘\nRefactoring...\n❌  Refactor failed: operation ExecuteStackRefactor is scaffolded but not implemented in this emulator\n' — this control proves refactoring itself works, isolating the earlier refusal to the combined property change

## Steps to Reproduce
Boot the cloudformation emulator
Call ExecuteStackRefactor and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
