Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.mstrx-empty-metric-names-list-reads-back-as-a-namespace-only-filter cloudwatch.mstrx-filter-budget-counts-filters-plus-metric-names-and-rejects-past-1000 cloudwatch.mstrx-firehose-arn-in-another-region-rejected cloudwatch.put-metric-stream-cross-account-firehose-arn-rejected cloudwatch.mstrx-include-only-stream-omits-exclude-filters-and-defaults-linked-accounts-false cloudwatch.op-contract-getMetricStream-dup cloudwatch.metric-stream-lastupdate-timestamp-reflects-start-stop cloudwatch.put-metric-stream-conflicting-include-exclude-filters-rejected cloudwatch.put-metric-stream-empty-statistics-configuration-preserved cloudwatch.put-metric-stream-invalid-output-format-rejected cloudwatch.concurrent-start-stop-metric-streams-race-resolves-deterministically
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.mstrx-empty-metric-names-list-reads-back-as-a-namespace-only-filter|run#10|AssertionError|assert got_empty == [{"Namespace": "AWS/EC2"}], (
Issue-Key: cloudwatch.mstrx-filter-budget-counts-filters-plus-metric-names-and-rejects-past-1000|run#10|AssertionError|assert msg == _BUDGET_REFUSAL, f"1 filter + 1000 names: message {msg!r}, expected {_BUDGET_REFUSAL!r}"
Issue-Key: cloudwatch.mstrx-firehose-arn-in-another-region-rejected|_error#7|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: cloudwatch.put-metric-stream-cross-account-firehose-arn-rejected|run#16|AssertionError|raise AssertionError("expected PutMetricStream to reject a cross-account FirehoseArn, but it succeeded")
Issue-Key: cloudwatch.mstrx-include-only-stream-omits-exclude-filters-and-defaults-linked-accounts-false|run#9|AssertionError|assert got.get("IncludeLinkedAccountsMetrics") is False, (
Issue-Key: cloudwatch.op-contract-getMetricStream-dup|run#16|AssertionError|assert message == "", f"expected an empty message, got {message!r}"
Issue-Key: cloudwatch.metric-stream-lastupdate-timestamp-reflects-start-stop|run#4|AssertionError|assert error == ("ResourceNotFoundException", ""), (
Issue-Key: cloudwatch.put-metric-stream-conflicting-include-exclude-filters-rejected|run#13|AssertionError|assert err.get("Message", "") == expected, f"expected {expected!r}, got {err.get('Message', '')!r}"
Issue-Key: cloudwatch.put-metric-stream-empty-statistics-configuration-preserved|run#22|AssertionError|assert "StatisticsConfigurations" not in resp_explicit_empty, (
Issue-Key: cloudwatch.put-metric-stream-invalid-output-format-rejected|run#5|AssertionError|assert err["Code"] == "ValidationError", f"expected ValidationError, got {err!r}"
Issue-Key: cloudwatch.concurrent-start-stop-metric-streams-race-resolves-deterministically|run#14|AssertionError|assert all(results), f"a concurrent Start/StopMetricStreams call was rejected instead of racing cleanly: {results}"
Issue-Commit: d26de989ce4778e024797c8509c4713b425f0550

# PutMetricStream/GetMetricStream: ARN scope is never validated, empty-collection normalization and the IncludeLinkedAccountsMetrics default are wrong, and four refusals carry the wrong code/message (10 causes)

> _Note: this issue was identified against the code base at commit `d26de989ce4778e024797c8509c4713b425f0550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricStream

## Description
Ten net-new root causes on the metric-stream configuration surface. Smithy: PutMetricStreamInput { Name @required, IncludeFilters, ExcludeFilters, FirehoseArn @required, RoleArn @required, OutputFormat @required (MetricStreamOutputFormat enum: json|opentelemetry0.7|opentelemetry1.0), Tags, StatisticsConfigurations, IncludeLinkedAccountsMetrics }; modeled errors ConcurrentModificationException (429), InvalidParameterCombinationException (400), InvalidParameterValueException (400), MissingRequiredParameterException (400), InternalServiceFault (500) — no ValidationException. GetMetricStreamOutput echoes IncludeFilters/ExcludeFilters/StatisticsConfigurations/IncludeLinkedAccountsMetrics and models ResourceNotFoundException (404).

Cause 1 of 10: an exclude/include filter whose MetricNames list is explicitly empty is not normalized away.
AWS: an empty MetricNames list is equivalent to the omitted form; GetMetricStream reads the filter back with the member dropped, i.e. [{'Namespace': 'AWS/EC2'}].
Emulator: GetMetricStream returns [{'Namespace': 'AWS/EC2', 'MetricNames': []}].
Why: a client comparing a read-back configuration against the one it submitted (drift detection, CFN/Terraform-style diffing) sees a permanent spurious difference.
Fix: empty-collection normalization in the PutMetricStream handler / metric-stream representation.
Traces: cloudwatch.mstrx-empty-metric-names-list-reads-back-as-a-namespace-only-filter.

Cause 2 of 10: the filter-budget refusal message is rewritten.
AWS: the total of filters plus metric names is capped at 1000 and the refusal names the member and the cap in the documented 'ExcludeFilters greater than 1,000' form.
Emulator: message 'ExcludeFilters can name at most 1000 namespaces and metrics in total; got 1001'.
Why: the member-and-cap form is what clients parse to attribute the failure.
Fix: the refusal message in PutMetricStream filter-budget validation.
Traces: cloudwatch.mstrx-filter-budget-counts-filters-plus-metric-names-and-rejects-past-1000.

Cause 3 of 10: a FirehoseArn in another region is accepted.
AWS: PutMetricStream refuses a FirehoseArn whose region is not the stream's region, 400 InvalidParameterValue. Recorded behavior: a refusal.
Emulator: the call succeeds and the stream is created.
Why: a cross-region destination that AWS would never wire up looks healthy, so a misconfiguration is discovered only in production.
Fix: FirehoseArn region validation in the PutMetricStream handler.
Traces: cloudwatch.mstrx-firehose-arn-in-another-region-rejected.

Cause 4 of 10: a FirehoseArn in another account is accepted.
AWS: PutMetricStream refuses a cross-account FirehoseArn. Recorded behavior: a refusal.
Emulator: the call succeeds.
Why: same as cause 3, for the account dimension — a stream is created that can never deliver.
Fix: FirehoseArn account validation in the PutMetricStream handler.
Traces: cloudwatch.put-metric-stream-cross-account-firehose-arn-rejected.

Cause 5 of 10: an omitted IncludeLinkedAccountsMetrics is not materialized to its default.
AWS: the member is a boolean whose documented default is false; GetMetricStream on a stream created without it reads back IncludeLinkedAccountsMetrics=False.
Emulator: GetMetricStream omits the member entirely (None).
Why: a client reading the flag to decide whether linked-account metrics are streamed gets no value and must guess.
Fix: default materialization in the PutMetricStream handler / GetMetricStream serializer.
Traces: cloudwatch.mstrx-include-only-stream-omits-exclude-filters-and-defaults-linked-accounts-false.

Cause 6 of 10: GetMetricStream's not-found error carries a message where AWS sends an empty one.
AWS: GetMetricStream on a name that does not exist answers 404 ResourceNotFoundException with an EMPTY Message — this operation's not-found is message-less.
Emulator: answers ResourceNotFoundException with 'the metric stream <name> does not exist'.
Why: a client that asserts on or logs the message (and the error-family contract that distinguishes CloudWatch's message-less not-found from its messaged ones) diverges.
Fix: the not-found error construction in the GetMetricStream handler.
Traces: cloudwatch.op-contract-getMetricStream-dup, cloudwatch.metric-stream-lastupdate-timestamp-reflects-start-stop.

Cause 7 of 10: the IncludeFilters/ExcludeFilters mutual-exclusion message is rewritten.
AWS: refuses with 'IncludeFilters and ExcludeFilters cannot both be present'.
Emulator: 'IncludeFilters and ExcludeFilters cannot be used in the same operation'.
Why: message-level divergence on a rule both the model and the recording state, so clients asserting the refusal text diverge.
Fix: the refusal message in PutMetricStream filter-combination validation.
Traces: cloudwatch.put-metric-stream-conflicting-include-exclude-filters-rejected.

Cause 8 of 10: an explicitly empty StatisticsConfigurations is preserved instead of normalized away.
AWS: an explicit empty list normalizes to absent; GetMetricStream omits StatisticsConfigurations.
Emulator: GetMetricStream returns StatisticsConfigurations=[].
Why: same read-back/diff problem as cause 1.
Fix: empty-collection normalization in the PutMetricStream handler / metric-stream representation.
Traces: cloudwatch.put-metric-stream-empty-statistics-configuration-preserved.

Cause 9 of 10: an out-of-enum OutputFormat is refused with an unmodeled ValidationException.
AWS: OutputFormat is the MetricStreamOutputFormat enum and PutMetricStream models no ValidationException; an out-of-enum value is refused 400 with a modeled validation error code.
Emulator: answers ValidationException "1 validation error detected: Value 'not-a-real-format' at 'outputFormat' failed to satisfy constraint: Member must satisfy enum value set: [json, opentelemetry0.7, opentelemetry1.0]".
Why: an error code the operation does not model, so SDK error mapping and typed handlers never match it.
Fix: enum rejection for PutMetricStream must surface as the operation's modeled validation error.
Traces: cloudwatch.put-metric-stream-invalid-output-format-rejected.

Cause 10 of 10: concurrent StartMetricStreams/StopMetricStreams calls are refused instead of racing to a deterministic outcome.
AWS: concurrent start/stop calls on the same stream all succeed and converge on one final state (ConcurrentModificationException, 429, is modeled but is not raised for this benign race). Recorded behavior: all eight concurrent calls succeed.
Emulator: all eight concurrent calls are rejected ([False x8]).
Why: a client or control loop that toggles a stream from more than one worker gets spurious failures where AWS converges.
Fix: concurrency handling in the StartMetricStreams/StopMetricStreams handlers and the metric-stream state.
Traces: cloudwatch.concurrent-start-stop-metric-streams-race-resolves-deterministically.

## Evidence
Cause 1: "AssertionError: an exclude filter sent with an EMPTY MetricNames list must read back with the member dropped, identical to the omitted form; got [{'Namespace': 'AWS/EC2', 'MetricNames': []}]".
Cause 2: "AssertionError: 1 filter + 1000 names: message 'ExcludeFilters can name at most 1000 namespaces and metrics in total; got 1001', expected 'ExcludeFilters greater than 1,000'".
Cause 3: "AssertionError: the call succeeded; real AWS rejects it, so there is no error code" (check_cloudwatch_mstrx_firehose_arn_in_another_region_rejected.py:34).
Cause 4: "AssertionError: expected PutMetricStream to reject a cross-account FirehoseArn, but it succeeded".
Cause 5: "AssertionError: an omitted IncludeLinkedAccountsMetrics must read back as the documented default False, not absent; got None".
Cause 6: "AssertionError: expected an empty message, got 'the metric stream sim2real-behav-cw-msg-missing-... does not exist'"; "AssertionError: GetMetricStream on a nonexistent metric stream name returned ('ResourceNotFoundException', 'the metric stream ... does not exist'), expected that exact ResourceNotFoundException".
Cause 7: "AssertionError: expected 'IncludeFilters and ExcludeFilters cannot both be present', got 'IncludeFilters and ExcludeFilters cannot be used in the same operation'".
Cause 8: "AssertionError: expected StatisticsConfigurations to be absent for an explicit empty list (matching real AWS's normalization), got []".
Cause 9: "AssertionError: expected ValidationError, got {'Message': \"1 validation error detected: Value 'not-a-real-format' at 'outputFormat' failed to satisfy constraint: Member must satisfy enum value set: [json, opentelemetry0.7, opentelemetry1.0]\", 'Code': 'ValidationException'}".
Cause 10: "AssertionError: a concurrent Start/StopMetricStreams call was rejected instead of racing cleanly: [False, False, False, False, False, False, False, False]".

## Steps to Reproduce
1. PutMetricStream with ExcludeFilters=[{'Namespace':'AWS/EC2','MetricNames':[]}]; GetMetricStream -> MetricNames:[] is still present.
2. PutMetricStream with 1 filter + 1000 metric names -> refusal message is not the 'ExcludeFilters greater than 1,000' form.
3. PutMetricStream with a FirehoseArn in another region -> succeeds.
4. PutMetricStream with a FirehoseArn in another account -> succeeds.
5. PutMetricStream without IncludeLinkedAccountsMetrics; GetMetricStream -> member absent instead of False.
6. GetMetricStream(Name='does-not-exist') -> ResourceNotFoundException with a non-empty Message.
7. PutMetricStream with both IncludeFilters and ExcludeFilters -> wrong refusal message.
8. PutMetricStream with StatisticsConfigurations=[]; GetMetricStream -> [] instead of absent.
9. PutMetricStream with OutputFormat='not-a-real-format' -> ValidationException.
10. Issue 8 concurrent StartMetricStreams/StopMetricStreams calls on one stream -> all are rejected.
