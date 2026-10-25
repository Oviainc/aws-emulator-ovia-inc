Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.describe-stack-refactor-not-found-error cloudformation.stack-refactor-actions-ordering cloudformation.stack-refactor-lifecycle
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|CreateStackRefactor|NotImplemented
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# Implement cloudformation CreateStackRefactor: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `461d317fc71f56ed8ab822edc66618d36d651bfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStackRefactor

## Description
`CreateStackRefactor` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/create_stack_refactor.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `CreateStackRefactor` call, so each one unblocks only when `CreateStackRefactor` works: `cloudformation.describe-stack-refactor-not-found-error`, `cloudformation.stack-refactor-actions-ordering`, `cloudformation.stack-refactor-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/create_stack_refactor.rs`. Replaying the corpus, the emulator refused the `CreateStackRefactor` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_cloudformation_describe_stack_refactor_not_found_error.py", line 47, in run
    raise AssertionError(f"CreateStackRefactor rejected a well-formed refactor request: {exc}") from exc
AssertionError: CreateStackRefactor rejected a well-formed refactor request: An error occurred (NotImplemented) when calling the CreateStackRefactor operation: operation CreateStackRefactor is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cloudformation emulator
Call CreateStackRefactor and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
