Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda.metrics-lambda-throwing-handler-increments-both-invocations-and-errors cloudwatch__lambda.alias-resource-dimension-alarm-isolates-failing-alias cloudwatch__lambda.metrics-lambda-reserved-zero-throttles-without-invocations-rejected cloudwatch__lambda.throttles-alarm-breaches-on-reserved-zero-healthy-twin-ok cloudwatch__lambda.duration-maximum-alarm-breaches-on-slow-handler-healthy-twin-ok cloudwatch__lambda.metrics-lambda-async-retry-meters-every-attempt-but-each-event-once cloudwatch__lambda.metrics-lambda-post-runtime-extensions-duration-exists-only-with-an-extension cloudwatch__lambda.obs-lambda-1-provisioned-concurrency-metrics-split-served-from-spillover cloudwatch__lambda.metrics-lambda-alias-resource-dimension-isolates-alias-and-version-traffic
Issue-Key: cloudwatch__lambda.metrics-lambda-throwing-handler-increments-both-invocations-and-errors|_poll_until#11|AssertionError|raise AssertionError(f"{description} did not happen within {timeout_s}s; last read {last!r}")
Issue-Key: cloudwatch__lambda.alias-resource-dimension-alarm-isolates-failing-alias|_poll#16|AssertionError|raise AssertionError(f"timed out after {budget_s:.0f}s waiting for {what}; last={last!r}")
Issue-Key: cloudwatch__lambda.metrics-lambda-reserved-zero-throttles-without-invocations-rejected|_poll_until#14|AssertionError|raise AssertionError(f"{description} did not happen within {timeout_s}s; last read {last!r}")
Issue-Key: cloudwatch__lambda.throttles-alarm-breaches-on-reserved-zero-healthy-twin-ok|_poll#18|AssertionError|raise AssertionError(f"timed out after {budget_s:.0f}s waiting for {what}; last={last!r}")
Issue-Key: cloudwatch__lambda.duration-maximum-alarm-breaches-on-slow-handler-healthy-twin-ok|_poll#21|AssertionError|raise AssertionError(f"timed out after {budget_s:.0f}s waiting for {what}; last={last!r}")
Issue-Key: cloudwatch__lambda.metrics-lambda-async-retry-meters-every-attempt-but-each-event-once|_poll_until#11|AssertionError|raise AssertionError(f"{description} did not happen within {timeout_s}s; last read {last!r}")
Issue-Key: cloudwatch__lambda.metrics-lambda-post-runtime-extensions-duration-exists-only-with-an-extension|_poll_until#15|AssertionError|raise AssertionError(f"{description} did not happen within {timeout_s}s; last read {last!r}")
Issue-Key: cloudwatch__lambda.obs-lambda-1-provisioned-concurrency-metrics-split-served-from-spillover|_poll_until#24|AssertionError|raise AssertionError(f"{description} did not happen within {timeout_s}s; last read {last!r}")
Issue-Key: cloudwatch__lambda.metrics-lambda-alias-resource-dimension-isolates-alias-and-version-traffic|_poll_until#21|AssertionError|raise AssertionError(f"{description} did not happen within {timeout_s}s; last read {last!r}")
Issue-Commit: c33038d6637516191c9271b81207503148b3a194

# cloudwatch__lambda: 7 net-new causes — AWS/Lambda publishes only Invocations: Errors, Throttles, Duration, the async-event counters, PostRuntimeExtensionsDuration and the provisioned-concurrency family are all absent, and per-qualifier series carry no ExecutedVersion dimension

> _Note: this issue was identified against the code base at commit `c33038d6637516191c9271b81207503148b3a194`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 7: the AWS/Lambda Errors series is never published.
AWS: an invocation whose handler raises increments Errors alongside Invocations under {FunctionName} (and under the per-qualifier dimension sets), so a throwing handler yields both counts and an alarm over Errors — including one scoped to a failing alias by the Resource dimension — reaches ALARM. Recorded behavior only for the series content; Smithy (cloudwatch 2010-08-01) supplies the alarm contract.
Emulator: Invocations arrives at the expected count while Errors reads None for the same function and window, and an alarm over alias-scoped Errors never leaves OK for 300 s.
Why: function failure is invisible. The most common Lambda alarm can never fire, and an error-rate ratio over Errors/Invocations is undefined.
Fix: the Lambda invoke path's metric emission — publish Errors on a failed invocation, in every dimension set in which Invocations is published.
Failing checks: cloudwatch__lambda.metrics-lambda-throwing-handler-increments-both-invocations-and-errors, cloudwatch__lambda.alias-resource-dimension-alarm-isolates-failing-alias.

Cause 2 of 7: the AWS/Lambda Throttles series is never published.
AWS: an invoke rejected because reserved concurrency is exhausted (ReservedConcurrentExecutions=0 refuses every invoke) increments Throttles under {FunctionName} without incrementing Invocations, so an alarm over Throttles reaches ALARM while a healthy twin stays OK.
Emulator: Throttles reads None after the refused invokes ('reaching 3 did not happen within 240.0s; last read None') and the Throttles alarm stays OK for 300 s.
Why: throttling — the signal that tells a customer to raise concurrency — is unobservable, so capacity problems present as silent latency rather than a metric.
Fix: the Lambda admission-control path must emit Throttles when it refuses an invoke.
Failing checks: cloudwatch__lambda.metrics-lambda-reserved-zero-throttles-without-invocations-rejected, cloudwatch__lambda.throttles-alarm-breaches-on-reserved-zero-healthy-twin-ok.

Cause 3 of 7: the AWS/Lambda Duration series is never published.
AWS: every completed invocation records Duration (milliseconds) under {FunctionName}, so Maximum/Average/p-statistics over it agree with the REPORT line of each invoke and a Duration-Maximum alarm distinguishes a slow handler from a fast twin.
Emulator: no Duration datapoint appears for the function at all ('timed out after 300s waiting for a real AWS/Lambda Duration datapoint under FunctionName=...; last=[]').
Why: latency is unmeasurable. Duration alarms and dashboards — and the timeout-risk signal they carry — never have data.
Fix: the Lambda invoke completion path must record Duration for each invocation.
Failing check: cloudwatch__lambda.duration-maximum-alarm-breaches-on-slow-handler-healthy-twin-ok.

Cause 4 of 7: the asynchronous-invocation counters are never published.
AWS: an async (Event) invoke increments AsyncEventsReceived when accepted, Invocations/Errors for every ATTEMPT (so a retried failure counts twice) and AsyncEventsDropped once per event that exhausts its retries — i.e. attempts and events are counted on different series.
Emulator: AsyncEventsReceived, AsyncEventsDropped and Errors all read None, and Invocations reports 2 where 4 attempts were made, so the retry is neither metered nor (apparently) performed.
Why: a customer cannot see that async events were accepted, retried or dropped, which is the only way to detect event loss on the Event invocation type.
Fix: the asynchronous invocation queue/retry component must emit AsyncEventsReceived/AsyncEventsDropped and count each delivery attempt in Invocations/Errors.
Failing check: cloudwatch__lambda.metrics-lambda-async-retry-meters-every-attempt-but-each-event-once.

Cause 5 of 7: PostRuntimeExtensionsDuration is never published.
AWS: a function with a registered external extension records PostRuntimeExtensionsDuration under {FunctionName} (the time extensions run after the handler returns); a function with no extension publishes no such series — the metric's presence is the differential.
Emulator: the series reads None for the function WITH an extension, so the differential cannot exist.
Why: extension overhead is invisible, so a customer cannot attribute billed duration between handler and extensions.
Fix: the Lambda extensions/runtime lifecycle component must record PostRuntimeExtensionsDuration when extensions run.
Failing check: cloudwatch__lambda.metrics-lambda-post-runtime-extensions-duration-exists-only-with-an-extension.

Cause 6 of 7: the provisioned-concurrency metric family is never published.
AWS: invoking an alias with provisioned concurrency splits the traffic across ProvisionedConcurrencyInvocations (served from the warm pool), ProvisionedConcurrencySpilloverInvocations (served on demand beyond the pool) and ProvisionedConcurrencyUtilization, in addition to Invocations.
Emulator: Invocations reads 2.0 while 'pci', 'spill' and 'util' are all empty for 300 s.
Why: a customer who pays for provisioned concurrency cannot see whether it is being used or spilling, so the sizing decision the feature exists for cannot be made.
Fix: the provisioned-concurrency admission path must label each invocation and emit the three provisioned-concurrency series.
Failing check: cloudwatch__lambda.obs-lambda-1-provisioned-concurrency-metrics-split-served-from-spillover.

Cause 7 of 7: per-qualifier series are published without the ExecutedVersion dimension.
AWS: an invoke through an alias publishes series under {FunctionName}, {FunctionName, Resource=<fn>:<alias>} AND {FunctionName, ExecutedVersion=<n>, Resource=<fn>:<alias>} — the ExecutedVersion-bearing set is how weighted-alias traffic is attributed to the version that actually ran.
Emulator: the {FunctionName}, {FunctionName, Resource} and {FunctionName, Resource=<version>} sets are present with correct counts, but every {FunctionName, ExecutedVersion, Resource} series reads None (as does every Errors series).
Why: with a weighted alias a customer cannot tell which version served the traffic or which version is failing, so canary/blue-green rollouts cannot be judged from metrics.
Fix: the Lambda metric emission's dimension-set construction — add the ExecutedVersion-qualified dimension set for alias-routed invocations.
Failing check: cloudwatch__lambda.metrics-lambda-alias-resource-dimension-isolates-alias-and-version-traffic.

## Evidence
Cause 1 — Expected {('Invocations', fn_err): 5.0, ('Errors', fn_err): 3.0, ('Invocations', fn_ok): 2.0}; actual last read {('Invocations', fn_err): 5.0, ('Errors', fn_err): None, ('Invocations', fn_ok): 2.0}. Alias arm: 'timed out after 300s waiting for alarm ... to reach ALARM from real AWS/Lambda Errors on alias ...; last={...: \'OK\', ...: \'OK\'}'.
Cause 2 — 'AssertionError: AWS/Lambda Throttles for [FunctionName=...] reaching 3 did not happen within 240.0s; last read None'; 'timed out after 300s waiting for alarm ... to reach ALARM from real AWS/Lambda Throttles ...'.
Cause 3 — 'AssertionError: timed out after 300s waiting for a real AWS/Lambda Duration datapoint under FunctionName=...; last=[]'.
Cause 4 — Expected {AsyncEventsReceived: 2, AsyncEventsDropped: 2, Invocations: 4, Errors: 4} on the retrying function; actual {AsyncEventsReceived: None, AsyncEventsDropped: None, Invocations: 2.0, Errors: None}.
Cause 5 — 'AssertionError: PostRuntimeExtensionsDuration for [FunctionName=...] did not happen within 240.0s; last read None'.
Cause 6 — 'AssertionError: the live alias\'s PC series reaching 2 sequential invokes did not happen within 300.0s; last read {\'inv\': [2.0], \'pci\': [], \'spill\': [], \'util\': []}'.
Cause 7 — last read shows ('Invocations', (FunctionName, Resource=<fn>:blue)) = 3.0 but ('Invocations', (FunctionName, ExecutedVersion=1, Resource=<fn>:blue)) = None and every ('Errors', ...) = None.

## Steps to Reproduce
Cause 1: invoke a throwing handler 3 of 5 times; poll AWS/Lambda Invocations and Errors for {FunctionName}. Cause 2: PutFunctionConcurrency(ReservedConcurrentExecutions=0), invoke 3 times, poll Throttles. Cause 3: invoke a handler that sleeps ~1 s; poll Duration (Maximum) for {FunctionName}. Cause 4: async-invoke (InvocationType='Event') a failing handler with retries configured; poll AsyncEventsReceived/AsyncEventsDropped/Invocations/Errors. Cause 5: deploy a function with an external extension and invoke it; poll PostRuntimeExtensionsDuration. Cause 6: PutProvisionedConcurrencyConfig on an alias, invoke it twice; poll ProvisionedConcurrencyInvocations/SpilloverInvocations/Utilization. Cause 7: create two versions and a weighted alias, invoke through the alias, then ListMetrics/GetMetricStatistics for AWS/Lambda Invocations and inspect the {FunctionName, ExecutedVersion, Resource} dimension sets.
