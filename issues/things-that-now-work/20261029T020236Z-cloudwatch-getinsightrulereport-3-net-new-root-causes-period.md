Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.invalid-parameter-value-family cloudwatch.missing-required-parameter-family cloudwatch.get-insight-rule-report-returns-metrics-for-active-rule cloudwatch.deep-cloudwatch-3-cwinsightrank-sum-and-maximum-orderings-disagree
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.invalid-parameter-value-family|_err#24|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudwatch.missing-required-parameter-family|run#15|AssertionError|assert error == ("InvalidParameterValue", "INVALID_PERIOD: A Period must be a multiple of 60 seconds, from 60 to 86400."), (
Issue-Key: cloudwatch.get-insight-rule-report-returns-metrics-for-active-rule|run#25|AssertionError|assert report["AggregationStatistic"] == "SampleCount", (
Issue-Key: cloudwatch.deep-cloudwatch-3-cwinsightrank-sum-and-maximum-orderings-disagree|run#36|AssertionError|assert got_max == expected_max, (
Issue-Commit: 7b0476a706ac79cc0f7894740758ad8ab457f053

# CloudWatch GetInsightRuleReport: 3 net-new root causes — Period is never validated (and existence is checked first), AggregationStatistic echoes AggregateOn instead of the statistic name, and OrderBy=Maximum returns window sums

> _Note: this issue was identified against the code base at commit `7b0476a706ac79cc0f7894740758ad8ab457f053`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInsightRuleReport

## Description
GetInsightRuleReport now answers (it is no longer refused), but the implemented handler diverges from AWS in three independent ways. 3 causes are bundled here; each must be closed before submitting.

Note on existing issues: the open issue 20261025T053543Z-cloudwatch-getinsightrulereport-stub.md still describes GetInsightRuleReport as an unmodified scaffold answering HTTP 501 NotImplemented, and the resolved 20261025T070825Z-cloudwatch-contributor-insights-reporting-is-inert-a-rule-over.md covered the report being inert. Neither covers the causes below: in this replay the operation answers HTTP 200 with a populated report, and these are fidelity defects of that answer.

Cause 1 of 3: GetInsightRuleReport never validates Period, and when it does refuse it checks rule existence before the parameter.
AWS: Smithy: GetInsightRuleReport.Period is @required integer with @range min 1; modeled error InvalidParameterValueException -> awsQueryError InvalidParameterValue, HTTP 400 (also modeled: MissingRequiredParameterException -> MissingParameter/400, ResourceNotFoundException -> ResourceNotFoundException/404). Recorded behavior: a Period that is not a multiple of 60 seconds in [60, 86400] is refused with HTTP 400, Error.Code 'InvalidParameterValue' and Error.Message 'INVALID_PERIOD: A Period must be a multiple of 60 seconds, from 60 to 86400.' — that message is entirely fixed text for this error class, it carries no value taken from the request. The recording also shows this parameter check runs BEFORE the rule lookup: the same Period=7 request naming a rule that does not exist is still refused with InvalidParameterValue/INVALID_PERIOD, never with ResourceNotFoundException.
Emulator: with an existing rule and Period=7 the call SUCCEEDS with HTTP 200 and a full report body (recorded reply: {'KeyLabels': ['$.requestId'], 'AggregationStatistic': 'Count', 'AggregateValue': 0.0, 'ApproximateUniqueCount': 0, 'Contributors': [], 'MetricDatapoints': []}). With a rule name that was never created and the same Period=7, it answers ('ResourceNotFoundException', 'Rule <rule-name> does not exist.') — i.e. it resolves the rule first and still never evaluates Period.
Why: a client that sends a sub-minute or non-multiple-of-60 Period gets a silently meaningless 200 report instead of the refusal AWS gives, so period-granularity bugs are invisible until production; and error-handling code that branches on InvalidParameterValue/INVALID_PERIOD sees ResourceNotFoundException instead, changing retry/abort decisions.
Fix: the GetInsightRuleReport handler's input validation, which must reject Period before the rule lookup.
Traces: cloudwatch.invalid-parameter-value-family (Period=7 on a live rule returned 200), cloudwatch.missing-required-parameter-family (Period=7 on an absent rule returned ResourceNotFoundException).

Cause 2 of 3: AggregationStatistic echoes the rule definition's AggregateOn value instead of the CloudWatch statistic name.
AWS: Smithy: GetInsightRuleReportOutput.AggregationStatistic (InsightRuleAggregationStatistic, string), documented 'Specifies whether this rule aggregates contributor data by COUNT or SUM'. Recorded behavior: for a rule whose RuleDefinition carries AggregateOn='Count', the report's AggregationStatistic is the CloudWatch statistic name 'SampleCount' (the AggregateOn literal is translated, not echoed); an AggregateOn='Sum' rule reports 'Sum'.
Emulator: returns AggregationStatistic 'Count' — the raw AggregateOn literal from the rule body (also visible in the 200 body quoted under Cause 1).
Why: a consumer that feeds AggregationStatistic straight into a CloudWatch statistic parameter, or compares it against the statistic names CloudWatch uses, gets a value CloudWatch does not accept and mis-labels the series it just read.
Fix: the GetInsightRuleReport handler's response serialization/mapping from the stored rule definition's AggregateOn to the reported statistic name.
Traces: cloudwatch.get-insight-rule-report-returns-metrics-for-active-rule.

Cause 3 of 3: OrderBy='Maximum' reports each contributor's window TOTAL as ApproximateAggregateValue, so Maximum and Sum rankings cannot disagree.
AWS: Smithy: GetInsightRuleReportInput.OrderBy documented 'Determines what statistic to use to rank the contributors. Valid values are Sum and Maximum.'; Contributors[].ApproximateAggregateValue (@required double) is 'an approximation of the aggregate value that comes from this contributor'. Recorded behavior: under OrderBy='Maximum' that aggregate is the contributor's largest value within a single Period, not the sum over the whole window, so a steady contributor with the larger total ranks BELOW a spiky contributor with the larger single-period peak; under OrderBy='Sum' the ordering is by window total.
Emulator: for a window containing a 'steady' contributor (three 20-byte events in each of four consecutive minutes) and a 'spiky' contributor (three 50-byte events in one minute), OrderBy='Maximum' returned per-contributor ApproximateAggregateValue {'spiky': 150.0, 'steady': 240.0} — steady's four-minute total — where AWS returns each contributor's largest per-period total {'steady': 60.0, 'spiky': 150.0}. The emulator therefore also ranks steady first under Maximum, inverting AWS's order.
Why: Contributor Insights exists to rank top talkers; a client asking for Maximum to find burst offenders is handed Sum's answer, so both the values and the ranking order are wrong and the two orderings can never disagree as AWS's do.
Fix: the insight-rule report aggregation in the GetInsightRuleReport handler/state — compute per-Period buckets per contributor and, for OrderBy='Maximum', report and rank by each contributor's maximum bucket value.
Traces: cloudwatch.deep-cloudwatch-3-cwinsightrank-sum-and-maximum-orderings-disagree.

## Evidence
Cause 1:
- cloudwatch.invalid-parameter-value-family [mismatch]: AssertionError: expected a rejection, but the call SUCCEEDED and returned {'KeyLabels': ['$.requestId'], 'AggregationStatistic': 'Count', 'AggregateValue': 0.0, 'ApproximateUniqueCount': 0, 'Contributors': [], 'MetricDatapoints': [], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}} for get_insight_rule_report(RuleName=<existing rule>, StartTime=now-10m, EndTime=now, Period=7). Expected HTTP 400 InvalidParameterValue, Message 'INVALID_PERIOD: A Period must be a multiple of 60 seconds, from 60 to 86400.'
- cloudwatch.missing-required-parameter-family [mismatch]: AssertionError: GetInsightRuleReport: Period=7 gave ('ResourceNotFoundException', 'Rule sim2real-behav-cwprobe-rule-<id> does not exist.'); expected ('InvalidParameterValue', 'INVALID_PERIOD: A Period must be a multiple of 60 seconds, from 60 to 86400.'). The rule name and the generated id in that message are per-run values; the requirement is the code and the fixed INVALID_PERIOD text.

Cause 2:
- cloudwatch.get-insight-rule-report-returns-metrics-for-active-rule [mismatch]: AssertionError: expected AggregationStatistic 'SampleCount' (AggregateOn='Count'), got Count. The rule was created with RuleDefinition {"Schema":{"Name":"CloudWatchLogRule","Version":1},"LogGroupNames":[G],"LogFormat":"JSON","Contribution":{"Keys":["$.ip"],"Filters":[]},"AggregateOn":"Count"} and RuleState=ENABLED, and the report did contain Contributors.

Cause 3:
- cloudwatch.deep-cloudwatch-3-cwinsightrank-sum-and-maximum-orderings-disagree [mismatch]: AssertionError: OrderBy=Maximum ApproximateAggregateValue per contributor is {'spiky': 150.0, 'steady': 240.0}, expected each contributor's largest per-period total {'steady': 60.0, 'spiky': 150.0}. 240.0 is exactly steady's 4 x (3 x 20) window sum, i.e. the Sum aggregate served for an OrderBy=Maximum request.

## Steps to Reproduce
Boot the emulator and point botocore cloudwatch + logs clients at it.

Cause 1:
1. cw.put_insight_rule(RuleName=R, RuleDefinition=json.dumps({"Schema":{"Name":"CloudWatchLogRule","Version":1},"AggregateOn":"Count","LogGroupNames":["/aws/lambda/*"],"LogFormat":"JSON","Contribution":{"Keys":["$.requestId"],"Filters":[]}})).
2. cw.get_insight_rule_report(RuleName=R, StartTime=now-10m, EndTime=now, Period=7) -> observe HTTP 200 with a report body; AWS refuses with 400 InvalidParameterValue 'INVALID_PERIOD: A Period must be a multiple of 60 seconds, from 60 to 86400.'
3. cw.get_insight_rule_report(RuleName=<never-created name>, StartTime=now-10m, EndTime=now, Period=7) -> observe ResourceNotFoundException 'Rule <name> does not exist.'; AWS still answers the INVALID_PERIOD InvalidParameterValue because the parameter is validated before the lookup.

Cause 2:
4. logs.create_log_group(G); logs.create_log_stream(G, S); cw.put_insight_rule(RuleName=R2, RuleState="ENABLED", RuleDefinition=json.dumps({"Schema":{"Name":"CloudWatchLogRule","Version":1},"LogGroupNames":[G],"LogFormat":"JSON","Contribution":{"Keys":["$.ip"],"Filters":[]},"AggregateOn":"Count"})).
5. logs.put_log_events(G, S, [{"timestamp": now_ms, "message": json.dumps({"ip": "10.0.0.1"})} ...]); poll cw.get_insight_rule_report(RuleName=R2, StartTime, EndTime, Period=60) until Contributors is non-empty.
6. Observe response['AggregationStatistic'] == 'Count'; AWS reports 'SampleCount' for AggregateOn='Count'.

Cause 3:
7. With a rule keyed on a host field over G, ingest events so one contributor ('steady') emits 3 x 20 units in each of four consecutive minutes and another ('spiky') emits 3 x 50 units in a single minute.
8. cw.get_insight_rule_report(RuleName=R3, StartTime, EndTime, Period=60, OrderBy="Maximum", Metrics=[...]) and read Contributors[].ApproximateAggregateValue.
9. Observe {'spiky': 150.0, 'steady': 240.0} (window sums, steady ranked first); AWS returns {'steady': 60.0, 'spiky': 150.0} and ranks spiky first, while OrderBy="Sum" ranks steady first.

Or replay the check ids in trace_ids.
