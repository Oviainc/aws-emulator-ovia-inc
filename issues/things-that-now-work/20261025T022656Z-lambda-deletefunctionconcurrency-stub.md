Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.delete-reserved-concurrency
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|DeleteFunctionConcurrency|NotImplemented
Issue-Commit: f04105fb2faf5f6bd82b29a86b394ae1fc089f7b

# Implement lambda DeleteFunctionConcurrency: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `f04105fb2faf5f6bd82b29a86b394ae1fc089f7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteFunctionConcurrency

## Description
`DeleteFunctionConcurrency` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/delete_function_concurrency.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteFunctionConcurrency` call, so each one unblocks only when `DeleteFunctionConcurrency` works: `lambda.delete-reserved-concurrency`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/delete_function_concurrency.rs`. Replaying the corpus, the emulator refused the `DeleteFunctionConcurrency` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteFunctionConcurrency operation: operation DeleteFunctionConcurrency is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the lambda emulator
Call DeleteFunctionConcurrency and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

## Update
The handler HAS since landed, in the fix for
`issues/things-that-now-work/20261025T022656Z-lambda-ten-read-representation-and-data-plane-divergences.md`:
one of that issue's recorded units calls `DeleteFunctionConcurrency` and was blocked on this
501, so the handler was implemented there (it clears the function's reserved concurrency and
answers 204, refusing a qualified name as `Get`/`PutFunctionConcurrency` do) and this file's
`//! SCAFFOLDED, NOT IMPLEMENTED:` census line was deleted. What is left for this issue is its
own unit, `lambda.delete-reserved-concurrency`, and a test that pins the operation's own rules.
