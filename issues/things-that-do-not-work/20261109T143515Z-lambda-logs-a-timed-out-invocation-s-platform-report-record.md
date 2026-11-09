Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__logs.log-cwl-system-warn-level-still-delivers-a-timeout-report
Issue-Key: lambda__logs.log-cwl-system-warn-level-still-delivers-a-timeout-report|_poll_until#11|AssertionError|raise AssertionError(f"{description} not observed within {timeout_s}s; last saw {last!r}")
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# A timed-out invocation's platform.report record is never delivered at system log level WARN

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the platform.report system record for a timed-out invocation is missing from the function's log group.
AWS: with LoggingConfig.SystemLogLevel=WARN the runtime still emits the platform report record for every invocation, including one killed by its timeout, so the log group holds a platform.report entry for that request id alongside the handler's traceback (Smithy lambda LoggingConfig.SystemLogLevel; the report record is a system-level, not application-level, record).
Emulator: the group holds the START, the handler's '[ERROR] RuntimeError: the invocation ran past its timeout' traceback and END for the timed-out request, but no platform.report for it — 'the timeout platform.report for <request id> in /aws/lambda/<function> not observed within 150.0s'.
Why: a client cannot measure billed duration, memory or the timeout outcome of the invocations that matter most; its log-based accounting silently omits timed-out requests.
Fix: system-record emission in the in-process Lambda runtime's logging path (emit platform.report on the timeout path, gated by SystemLogLevel).
Traces: lambda__logs.log-cwl-system-warn-level-still-delivers-a-timeout-report.

## Evidence
Expected: a platform.report record for the timed-out request id. Actual: 'AssertionError: the timeout platform.report for 9960041e-... in /aws/lambda/sim2real-behav-l4-<id> not observed within 150.0s; last saw [START..., '[ERROR] RuntimeError: the invocation ran past its timeout', traceback lines, END ...]'.

## Steps to Reproduce
Replay lambda__logs.log-cwl-system-warn-level-still-delivers-a-timeout-report: create a function with SystemLogLevel=WARN, invoke it so it exceeds its timeout and read the function's log group.
