Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit__codepipeline__glue__iam__lambda.refarch-glue-launcher-log-group-absent-until-first-invocation
Issue-Key: codecommit__codepipeline__glue__iam__lambda.refarch-glue-launcher-log-group-absent-until-first-invocation|_poll#24|Timeout|time.sleep(interval_s)
Issue-Commit: 8f45cbde1fc786a42a670207326215689e25a831

# codecommit__codepipeline__glue__iam__lambda: the implicit /aws/lambda/<function> log group never becomes observable after the first invocation, so the launcher log-group check never reaches a verdict

> _Note: this issue was identified against the code base at commit `8f45cbde1fc786a42a670207326215689e25a831`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One root cause.

Cause 1 of 1: the contradiction a Lambda log group is supposed to exhibit — named by `GetFunctionConfiguration.LoggingConfig.LogGroup` from creation, but absent from CloudWatch Logs until the function actually runs, then present with exactly one stream for that one invocation — never completes on the emulator: the check does not reach a verdict within its budget.

AWS: `GetFunctionConfiguration` reports `LoggingConfig.LogGroup` = `/aws/lambda/<function>` and `LogFormat` = the applied default `Text` from the moment the function is created. Before any invocation, `logs:DescribeLogGroups` with that exact prefix returns nothing and `logs:DescribeLogStreams` on it is refused `ResourceNotFoundException` with the message `The specified log group does not exist.` (fixed text; no variable parts). The FIRST invocation creates the group, and it then holds exactly one stream whose name is `YYYY/MM/DD/[$LATEST]<id>` — three slashes, the `[$LATEST]` version marker for an unpublished function, and the UTC date on which the invocation ran. Smithy: `logs:DescribeLogStreams` models `ResourceNotFoundException`; the creation of the group by the first invocation and the stream-name shape are recorded/documented behavior, not modeled.

Emulator: the check exhausts the suite's 60 s budget with no assertion verdict. Every assertion before the barrier is a hard assert that would have surfaced as a mismatch, so the run is sitting in one of the remaining waits: the bounded `CreateFunction` retry for the new role to become assumable, the `State`→`Active` poll, or — the only ones that depend on the behavior under test — the 180 s polls for `/aws/lambda/<function>` to appear in `DescribeLogGroups` after the one `Invoke` and for that group to list exactly one stream. Hypothesis: `Invoke` does not create the function's implicit log group (or its stream), so those polls can never settle. This is consistent with the same service family's confirmed Invoke-side gaps and with the three `cloudformation__iam__lambda__logs` checks that fail one step earlier on the missing `x-amzn-RequestId` header.

Why: "no log group for the launcher" is how an operator confirms a deploy action never fired. If the group never appears after a real invocation, that absence proves nothing about the function and the operator draws the opposite conclusion from a broken observation; and no invocation's output is ever readable.

Fix: the Invoke execution path's logging side — the first invocation must create `/aws/lambda/<function>` in CloudWatch Logs state and open one stream named `YYYY/MM/DD/[$LATEST]<id>` for it, and `DescribeLogGroups`/`DescribeLogStreams` must serve it.

Traces: codecommit__codepipeline__glue__iam__lambda.refarch-glue-launcher-log-group-absent-until-first-invocation.

## Evidence
Expected (AWS): after exactly one `Invoke`, `describe_log_groups(logGroupNamePrefix='/aws/lambda/<fn>')` returns exactly `['/aws/lambda/<fn>']`, and `describe_log_streams` on it returns exactly one stream named `YYYY/MM/DD/[$LATEST]<id>`.

Actual (emulator): `Error: Check timed out after 60.0s` — the check never produced a verdict. Its only behavior-dependent waits are `_poll(lambda: _groups(logs, group), lambda names: names == [group], timeout_s=180.0)` and the follow-on one-stream poll; the preceding asserts (`LoggingConfig.LogGroup`, `LogFormat == 'Text'`, the pre-invocation empty listing, and `('ResourceNotFoundException', 'The specified log group does not exist.')`) did not fire.

## Steps to Reproduce
1. Create an IAM role Lambda may assume and a python3.12 function (no `LoggingConfig` override).
2. Confirm `GetFunctionConfiguration` reports `LoggingConfig.LogGroup = /aws/lambda/<fn>` and `LogFormat = Text`, and that `DescribeLogGroups` with that exact prefix is empty while `DescribeLogStreams` raises `ResourceNotFoundException` / `The specified log group does not exist.`.
3. Wait for `State == Active`, then call `lambda:Invoke` once.
4. Poll `DescribeLogGroups` for `/aws/lambda/<fn>` — the group does not appear, and `DescribeLogStreams` keeps reporting it absent.
