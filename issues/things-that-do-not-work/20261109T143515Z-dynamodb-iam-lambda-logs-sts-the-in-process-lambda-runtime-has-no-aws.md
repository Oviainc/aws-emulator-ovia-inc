Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__iam__lambda__logs__sts.durable-execution-step-exhausts-retries-fails-execution dynamodb__iam__lambda__logs__sts.durable-execution-step-retry-recovers-and-replay-skips-completed-steps
Issue-Key: dynamodb__iam__lambda__logs__sts.durable-execution-step-exhausts-retries-fails-execution|run#9|AssertionError|assert error["ErrorType"] == "CallableRuntimeError", (
Issue-Key: dynamodb__iam__lambda__logs__sts.durable-execution-step-retry-recovers-and-replay-skips-completed-steps|run#7|AssertionError|assert final["Status"] == "SUCCEEDED", f"expected SUCCEEDED, got {final['Status']}: {final.get('Error')}"
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# The in-process Lambda runtime has no aws_durable_execution_sdk_python module, so every durable-execution handler fails with Runtime.ImportModuleError

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the Python runtime image the emulator serves lacks the durable-execution SDK the runtime is documented to provide.
AWS: a function written against the durable-execution programming model imports aws_durable_execution_sdk_python from the managed runtime; the execution then runs its steps, retries a failing step and, when retries are exhausted, fails with Error.ErrorType 'CallableRuntimeError' while a recovering run resumes and skips completed steps.
Emulator: the handler cannot even be imported — the invoke result is {'ErrorMessage': "Unable to import module 'lambda_function': No module named 'aws_durable_execution_sdk_python'", 'ErrorType': 'Runtime.ImportModuleError'} — so neither the retry-exhaustion error type nor the replay-skip behaviour is reachable.
Why: a client cannot run any durable-execution function; the failure is an import error from the emulator's own runtime rather than the handler's modelled error.
Fix: the in-process Python Lambda runtime's bundled libraries in the Lambda crate (provide aws_durable_execution_sdk_python) and the durable-execution step/retry state it drives.
Traces: both checks in trace_ids.

## Evidence
Expected: Error.ErrorType 'CallableRuntimeError' after retries are exhausted, and SUCCEEDED for the recovering run. Actual: both invokes return {'ErrorMessage': "Unable to import module 'lambda_function': No module named 'aws_durable_execution_sdk_python'", 'ErrorType': 'Runtime.ImportModuleError'}.

## Steps to Reproduce
Replay dynamodb__iam__lambda__logs__sts.durable-execution-step-exhausts-retries-fails-execution and .durable-execution-step-retry-recovers-and-replay-skips-completed-steps.
