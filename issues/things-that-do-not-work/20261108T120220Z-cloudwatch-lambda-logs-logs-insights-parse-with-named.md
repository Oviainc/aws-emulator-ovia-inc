Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__logs.metrics-from-logs-insights-parse-named-capture-agrees-with-extracted-metric
Issue-Key: cloudwatch__lambda__logs.metrics-from-logs-insights-parse-named-capture-agrees-with-extracted-metric|run#13|AssertionError|assert by_insights == _EXPECTED, (
Issue-Commit: 63b1f768cca526e685e8991a6e4ba2a43400c274

# cloudwatch/lambda/logs: Logs Insights `parse` with named captures emits a bogus empty-key group for non-matching records instead of excluding them

> _Note: this issue was identified against the code base at commit `63b1f768cca526e685e8991a6e4ba2a43400c274`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueryResults

## Description
One net-new root cause, grounded in a failing check the replay report marked neither ALREADY FILED nor OWNED.

Cause 1 of 1 — a CloudWatch Logs Insights query of the form `parse <pattern with named captures> | stats sum(...), count(...) by <captured field>` returns an extra row whose grouping key is the empty string and whose count covers every record in the group, instead of restricting the result to records the parse pattern actually matched.
AWS: recorded behavior only — Insights `parse` with named captures binds its fields only on records the pattern matches; records it does not match have the captured field unset and therefore contribute to no group of a subsequent `stats ... by <field>`. For log lines 'XFER alice 30', 'XFER bob 5', 'XFER alice 12' plus the decoys 'SKIP carol 99' and 'RETRY alice 1000', the result is exactly two rows — alice (sum 42, count 2) and bob (sum 5, count 1) — matching a positional metric filter '[kind=XFER, user, nbytes]' on the same lines. There is no row for carol and no row for an empty user.
Emulator: GetQueryResults returns three rows, [{'user': '', 'total': '0', 'n': '5'}, {'user': 'alice', 'total': '42', 'n': '2'}, {'user': 'bob', 'total': '5', 'n': '1'}] — a group keyed on an empty user whose count is every record in the stream, so unmatched records are swept into a synthetic bucket rather than dropped, and the count there includes the records that did match.
Why: a client reading an Insights result cannot trust the row set — a dashboard or alarm built on `parse | stats by` gets a phantom series with a null key and an inflated count, and the Insights answer disagrees with the metric filter over the same lines, which is precisely the cross-instrument agreement the query language guarantees.
Fix: the Logs Insights query engine — `parse` named-capture binding and the `stats ... by` grouping must exclude records on which the parse pattern did not match (unset grouping key => no group), rather than grouping them under an empty value.
Failing check: cloudwatch__lambda__logs.metrics-from-logs-insights-parse-named-capture-agrees-with-extracted-metric.

## Evidence
AssertionError: Insights parse/stats returned [{'user': '', 'total': '0', 'n': '5'}, {'user': 'alice', 'total': '42', 'n': '2'}, {'user': 'bob', 'total': '5', 'n': '1'}], expected per-user (total, n) {'alice': (42, 2), 'bob': (5, 1)}. The alice/bob rows are correct; the defect is the additional group with user '' and n covering all five records. The specific user names and byte values are the check's fixture data filling the template; the requirement is that non-matching records form no group.

## Steps to Reproduce
Create a log group fed by a Lambda that emits the lines 'XFER alice 30', 'XFER bob 5', 'XFER alice 12', 'SKIP carol 99', 'RETRY alice 1000'; put a metric filter with pattern '[kind=XFER, user, nbytes]' (metricValue $nbytes, dimension User=$user); then logs StartQuery over the group with a query that parses the XFER lines with named captures for user and nbytes and runs `stats sum(nbytes), count(*) by user`, and GetQueryResults -> expect exactly the alice (42,2) and bob (5,1) rows, observe an extra row keyed on an empty user with count 5.
