Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__logs.obs-logs-3-service-usage-and-emf-policy-metrics
Issue-Key: cloudwatch__logs.obs-logs-3-service-usage-and-emf-policy-metrics|_poll_until#31|AssertionError|raise AssertionError(f"{description} not reached within {timeout_s}s; last reading {last!r}")
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# AWS/Logs service-usage and account-level EMF/policy metrics are never published

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricData

## Description
Cause 1 of 1: CloudWatch Logs publishes none of its own usage metrics.
AWS: CloudWatch Logs publishes service-usage metrics (AWS/Usage CallCount per operation with Class/Resource/Service/Type dimensions, and the account-level error counters) so a client can see how many logs calls and errors an account made.
Emulator: every counter reads None after 200s — 'service-usage counters reaching their floors {logs_calls: 13, usage_calls: 13, logs_errors: 3, account_calls: 13, account_errors: 3} not reached within 200s; last reading {logs_calls: None, usage_calls: None, logs_errors: None, account_calls: None, account_errors: None}' — i.e. no usage series exists at all even after the calls were made.
Why: a client cannot monitor its own CloudWatch Logs API usage or error rate, and quota dashboards built on AWS/Usage are empty.
Fix: the AWS/Usage metric publisher for CloudWatch Logs API calls (request accounting in the logs crate feeding the cloudwatch metric store).
Traces: cloudwatch__logs.obs-logs-3-service-usage-and-emf-policy-metrics.

## Evidence
Expected: the usage counters reach their floors after the calls. Actual: 'AssertionError: service-usage counters reaching their floors {...} not reached within 200s; last reading {'logs_calls': None, 'usage_calls': None, 'logs_errors': None, 'account_calls': None, 'account_errors': None}'.

## Steps to Reproduce
Replay cloudwatch__logs.obs-logs-3-service-usage-and-emf-policy-metrics: make a known number of logs calls (some failing) and poll GetMetricData for the AWS/Usage call and error counters.
