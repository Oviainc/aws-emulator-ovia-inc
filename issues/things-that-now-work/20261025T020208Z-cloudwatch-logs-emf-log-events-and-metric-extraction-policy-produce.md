Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__logs.metric-extraction-policy-invalid-status-rejected cloudwatch__logs.metric-extraction-resumes-after-policy-deleted
Issue-Key: cloudwatch__logs.metric-extraction-policy-invalid-status-rejected|_poll_until_stable#25|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__logs.metric-extraction-resumes-after-policy-deleted|_poll_until_stable#21|Timeout|time.sleep(interval_s)
Issue-Commit: d26de989ce4778e024797c8509c4713b425f0550

# EMF log events and METRIC_EXTRACTION_POLICY produce no CloudWatch metric data, so extraction cannot be observed to run, be disabled or resume (1 cause)

> _Note: this issue was identified against the code base at commit `d26de989ce4778e024797c8509c4713b425f0550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAccountPolicy

## Description
One net-new root cause.

Cause 1 of 1: embedded-metric-format log events never materialize CloudWatch metric data, and a METRIC_EXTRACTION_POLICY's effect is therefore never observable.
AWS: PutLogEvents of EMF-formatted lines into a log group materializes the embedded metrics in CloudWatch without any PutMetricData call; a CloudWatch Logs account policy of type METRIC_EXTRACTION_POLICY whose selectionCriteria names a log group suppresses extraction for that group — the suppressed group's series stops gaining datapoints while the service-usage counter for disabled extraction registers — and DeleteAccountPolicy resumes extraction so the series gains datapoints again. Recorded behavior: both checks read back concrete extracted values (e.g. (11.0, 1.0) and, after resumption, (9.0, 1.0)) and a disabled-extraction counter of 1.0, each held stable over a window.
Emulator: both checks ingest their EMF events and then poll for the extracted datapoints with a declared _APPEAR_TIMEOUT_S=200 budget; they are killed at the harness 60 s cap because no extracted metric ever appears. With nothing extracted in the first place, neither the policy's suppression nor its removal is distinguishable from the baseline.
Why: EMF is the primary way serverless and container workloads emit custom metrics; with extraction producing nothing, every EMF-based metric, dashboard and alarm is empty, and a client that applies a METRIC_EXTRACTION_POLICY cannot tell whether it took effect. Open issue 20261024T150703Z-logs-cloudwatch-logs-metric-filters-emit-no-cloudwatch-metric.md reports the metric-filter half of this under service `logs`; it does not carry these replay keys and does not cover the account-policy extraction path.
Fix: EMF extraction from ingested log events into the cloudwatch metric store, gated by METRIC_EXTRACTION_POLICY in the CloudWatch Logs PutAccountPolicy/DeleteAccountPolicy handlers, plus the service-usage counter for disabled extraction.
Traces: cloudwatch__logs.metric-extraction-policy-invalid-status-rejected, cloudwatch__logs.metric-extraction-resumes-after-policy-deleted.

## Evidence
Both checks: "Error: Check timed out after 60.0s" while in `_poll_until_stable(_present, window_s=_STABLE_WINDOW_S, timeout_s=_APPEAR_TIMEOUT_S, ...)` / `_poll_until_stable(_phase1, ...)`. The assertions they never reach: `assert extracted == (11.0, 1.0)`, `assert disabled[0] == 1.0`, `assert after == (9.0, 1.0)`, `assert during is None`. The checks also confirm ingestion itself was clean: `assert "rejectedLogEventsInfo" not in r, f"EMF PutLogEvents into {g!r} rejected: {r!r}"`.

## Steps to Reproduce
1. CreateLogGroup/CreateLogStream for two groups; PutAccountPolicy(policyType='METRIC_EXTRACTION_POLICY', selectionCriteria naming one group).
2. PutLogEvents EMF-formatted lines into both groups (no rejectedLogEventsInfo is returned).
3. Poll GetMetricStatistics for the embedded metric of the un-suppressed group -> no datapoint ever appears; the check hangs to the 60 s cap.
4. DeleteAccountPolicy and PutLogEvents more EMF lines -> still no datapoints, so resumption is not observable.
