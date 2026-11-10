Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__kms__lambda__logs__sts.durable-execution-kms-disabled-still-serves-metadata
Issue-Key: iam__kms__lambda__logs__sts.durable-execution-kms-disabled-still-serves-metadata|run#7|AssertionError|assert enabled["Status"] == "SUCCEEDED", (
Issue-Commit: 7f92290cd08a3e8e509afee1d943cf5282438c9c

# A durable-execution invoke ends Status FAILED where AWS ends SUCCEEDED, so the disabled-CMK metadata-only contract is unreachable

> _Note: this issue was identified against the code base at commit `7f92290cd08a3e8e509afee1d943cf5282438c9c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause.

Cause 1 of 1: a durable-execution function invoked RequestResponse never completes its step successfully — the execution settles FAILED, so every later read of the execution is read against a failed run.

AWS: a Python 3.13 function created with DurableConfig={ExecutionTimeout, KMSKeyArn} and a handler written against the managed durable-execution programming model (import aws_durable_execution_sdk_python; @durable_step; context.step(...)) is invoked against a published version with a DurableExecutionName; the invoke returns a DurableExecutionArn and the execution runs its one checkpointed step and reaches a terminal Status of SUCCEEDED. GetDurableExecution(IncludeExecutionData=False) then reports Status SUCCEEDED, ExecutionDataIncluded=false, DurableConfig.KMSKeyArn equal to the configured key, and GetDurableExecutionHistory(IncludeExecutionData=False) runs from ExecutionStarted to ExecutionSucceeded. Smithy: lambda#GetDurableExecution output has required Status -> ExecutionStatus (enum) and ExecutionDataIncluded, and models KMSDisabledException/KMSAccessDeniedException/KMSInvalidStateException/KMSNotFoundException (502) for the data-bearing path; the SUCCEEDED/FAILED values themselves are recorded behavior.
Emulator: the execution settles terminal with Status 'FAILED'. Because the run never succeeds, none of the behavior the check exists to pin — that with the execution's CMK disabled the metadata-only GetDurableExecution/GetDurableExecutionHistory (IncludeExecutionData=False) still succeed with Status SUCCEEDED, no InputPayload/Result, the original StartTimestamp and an ExecutionStarted→ExecutionSucceeded history, while the IncludeExecutionData=True calls fail KMSDisabledException — is reachable.
Why: no client can run a durable-execution function at all; a handler AWS completes is reported FAILED, so step/checkpoint semantics, the history event stream and every KMS-gated data/metadata distinction built on top of a completed execution are wrong or unobservable.
Hypothesis: the same cause as the already-open issue 20261109T143515Z-dynamodb-iam-lambda-logs-sts-the-in-process-lambda-runtime-has-no-aws.md — the in-process Python runtime bundles no aws_durable_execution_sdk_python, so the handler faults Runtime.ImportModuleError and the execution is recorded FAILED. That issue's replay keys name two dynamodb__iam__lambda__logs__sts checks and do not carry this check, so this failure is filed here and named to it rather than left uncovered; the replay output for this check reports only the terminal Status.
Fix: the Lambda crate's in-process Python runtime (the libraries it makes importable, so a durable handler can import the durable-execution SDK) and the durable-execution engine that drives an execution's steps and records its terminal Status and history events.
Traces: iam__kms__lambda__logs__sts.durable-execution-kms-disabled-still-serves-metadata.

## Evidence
From the behavioral replay of iam__kms__lambda__logs__sts.durable-execution-kms-disabled-still-serves-metadata [mismatch]:

AssertionError: the synchronously invoked execution should end SUCCEEDED, got 'FAILED'

- Expected: polling GetDurableExecution(DurableExecutionArn, IncludeExecutionData=False) to a terminal status yields Status 'SUCCEEDED' (with ExecutionDataIncluded=false and DurableConfig.KMSKeyArn = the configured key ARN).
- Actual: the terminal Status is 'FAILED'. The setup itself got through: CreateFunction with DurableConfig succeeded, the function reached Active, PublishVersion succeeded, and Invoke returned a DurableExecutionArn (the check raises before this assertion if it does not).
- Consequence: the disabled-key assertions that follow (metadata-only reads still SUCCEEDED with no InputPayload/Result and an ExecutionStarted→ExecutionSucceeded history, and KMSDisabledException on the IncludeExecutionData=True reads) were never evaluated.

## Steps to Reproduce
Replay iam__kms__lambda__logs__sts.durable-execution-kms-disabled-still-serves-metadata. In outline: create a Lambda execution role with AWSLambdaBasicExecutionRole and AWSLambdaBasicDurableExecutionRolePolicy; create a KMS key whose policy allows lambda.amazonaws.com and that role kms:GenerateDataKey/Decrypt/DescribeKey; CreateFunction (python3.13, handler importing aws_durable_execution_sdk_python with one @durable_step returned via context.step) with DurableConfig={ExecutionTimeout:900, KMSKeyArn:<key arn>}; PublishVersion; Invoke RequestResponse against that version with DurableExecutionName set; then poll GetDurableExecution(IncludeExecutionData=False) until the Status is terminal and read it.
