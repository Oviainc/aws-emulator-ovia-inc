Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__kms__lambda__logs__sts.durable-execution-kms-disabled-still-serves-metadata
Issue-Key: iam__kms__lambda__logs__sts.durable-execution-kms-disabled-still-serves-metadata|_setup#5|AssertionError|raise AssertionError("unreachable: Invoke response did not include a DurableExecutionArn")
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# A durable-execution Invoke response omits DurableExecutionArn

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the invoke of a durable-execution function returns no execution handle.
AWS: invoking a function configured for durable execution returns the execution's DurableExecutionArn in the response, which the caller then uses to read the execution's metadata (and that metadata is still served when the KMS key backing it is disabled).
Emulator: the response carries no DurableExecutionArn at all — the check cannot proceed and reports 'unreachable: Invoke response did not include a DurableExecutionArn'.
Why: a client has no handle to the execution it just started, so it cannot poll status, read metadata or correlate the run; the KMS-disabled-still-serves-metadata behaviour is unobservable.
Fix: the Invoke response serializer/state for durable-execution functions in the Lambda crate (populate the execution ARN).
Traces: iam__kms__lambda__logs__sts.durable-execution-kms-disabled-still-serves-metadata.

## Evidence
Expected: Invoke returns a DurableExecutionArn. Actual: 'AssertionError: unreachable: Invoke response did not include a DurableExecutionArn'.

## Steps to Reproduce
Replay iam__kms__lambda__logs__sts.durable-execution-kms-disabled-still-serves-metadata: create the durable-execution function, invoke it and read DurableExecutionArn from the response.
