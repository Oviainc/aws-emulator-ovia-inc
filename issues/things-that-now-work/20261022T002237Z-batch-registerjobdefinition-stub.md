Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__ecr__lambda__logs.refarch-batch-fargate-job-definition-without-execution-role-rejected
Issue-Key: batch__ecr__lambda__logs.refarch-batch-fargate-job-definition-without-execution-role-rejected|run#8|AssertionError|assert (code, message) == ('ClientException', expected), (
Issue-Commit: ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3

# Implement batch RegisterJobDefinition: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterJobDefinition

## Description
`RegisterJobDefinition` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/register_job_definition.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `RegisterJobDefinition` call, so each one unblocks only when `RegisterJobDefinition` works: `batch__ecr__lambda__logs.refarch-batch-fargate-job-definition-without-execution-role-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/register_job_definition.rs`. Replaying the corpus, the emulator refused the `RegisterJobDefinition` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_batch__ecr__lambda__logs_refarch_batch_fargate_job_definition_without_execution_role_rejected.py", line 44, in run
    assert (code, message) == ('ClientException', expected), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: registering a Fargate job definition with no executionRoleArn raised 'NotImplemented': 'operation RegisterJobDefinition is scaffolded but not implemented in this emulator', expected ClientException 'Error executing request, Exception : executionRoleArn is required for Fargate jobs., RequestId: None'

## Steps to Reproduce
Boot the batch emulator
Call RegisterJobDefinition and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
