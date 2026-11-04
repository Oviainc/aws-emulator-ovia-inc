Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__logs.log-cwl-plumbing-lambda-envelope-carries-only-pattern-matched-lines
Issue-Key: lambda__logs.log-cwl-plumbing-lambda-envelope-carries-only-pattern-matched-lines|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Commit: b4ead8169e48e6047949699a772a41fd75a6bc4d

# A Lambda subscription filter's envelope is never delivered, so pattern-matched-only lines cannot be observed

> _Note: this issue was identified against the code base at commit `b4ead8169e48e6047949699a772a41fd75a6bc4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
Cause 1 of 1: a CloudWatch Logs subscription filter with a Lambda destination must invoke the function with a gzip+base64 'awslogs' envelope that carries only the log events matching the filter pattern.
AWS: recorded behavior only — the recording puts matching and non-matching lines into the source group, then reads the destination function's own log output to inspect the decoded envelope and confirm only the matching lines are inside; on AWS this lands inside the suite's 60s budget.
Emulator: the check is cut off with "Error: Check timed out after 60.0s" — PutSubscriptionFilter is accepted but the destination function is never invoked, so no envelope ever exists to inspect.
Why: a client that ships filtered log lines into a Lambda gets a filter that reports itself installed and a function that is never called, so neither the envelope shape nor the pattern filtering is observable.
Fix: the CloudWatch Logs subscription-filter delivery path — pattern evaluation against incoming events and the invocation of the Lambda destination with the awslogs envelope.
Traces: lambda__logs.log-cwl-plumbing-lambda-envelope-carries-only-pattern-matched-lines.

## Evidence
Expected (AWS): the destination function is invoked with an awslogs envelope containing only the pattern-matched log events.
Actual (emulator): 'Error: Check timed out after 60.0s' — the destination function is never invoked.

## Steps to Reproduce
1. Create a log group and a Lambda destination; AddPermission for logs.amazonaws.com.
2. PutSubscriptionFilter with a filter pattern naming that function.
3. PutLogEvents with matching and non-matching lines and poll the destination function's log group. Observe it is never invoked within 60s.
