Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.get-insight-rule-report-time-series-ordering
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.get-insight-rule-report-time-series-ordering|run#18|Timeout|time.sleep(10)
Issue-Commit: f29a5831c5957291c6cb768de44e48e5e558bd01

# Contributor Insights reporting is inert: a rule over a live log group never yields a report, and the check hangs to the suite cap before GetInsightRuleReport can answer (1 cause)

> _Note: this issue was identified against the code base at commit `f29a5831c5957291c6cb768de44e48e5e558bd01`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInsightRuleReport

## Description
Cause 1 of 1: the Contributor Insights read path produces no report for an ENABLED CloudWatchLogRule whose log group is actively ingesting matching JSON events, so a client that polls for contributor/datapoint data never converges and the replayed check burns its whole budget without ever obtaining a report. Failing check: cloudwatch.get-insight-rule-report-time-series-ordering.

AWS: GetInsightRuleReport is modeled on the CloudWatch service (Smithy: GetInsightRuleReport, input RuleName (@required, @length 1..128, @pattern ^[\x20-\x7E]+$), StartTime/EndTime (@required timestamp), Period (@required integer, @range min 1), optional MaxContributorCount/Metrics/OrderBy; output KeyLabels, AggregationStatistic, AggregateValue, ApproximateUniqueCount, Contributors, MetricDatapoints; modeled errors InvalidParameterValueException -> awsQueryError InvalidParameterValue/400, MissingRequiredParameterException -> MissingParameter/400, ResourceNotFoundException -> ResourceNotFoundException/404). The recorded behavior this check was built from is: after PutInsightRule creates an ENABLED rule keyed on $.ip over a log group that then receives matching JSON log events, GetInsightRuleReport over a window of at least one Period eventually returns a non-empty Contributors list containing the ingested key, and a MetricDatapoints series (populated only when Metrics asks for at least one statistic) that is strictly ascending by Timestamp with no duplicate timestamps. Windows shorter than one Period are refused with InvalidParameterValue carrying INVALID_TIME_RANGE; that refusal is the only thing the recording shows as an error on this path.

Emulator: every setup call is accepted (logs CreateLogGroup, CreateLogStream, cloudwatch PutInsightRule with RuleState=ENABLED, and repeated logs PutLogEvents all succeed), and then the check's convergence loop never reaches a successful report: the replay verdict is `mismatch` with detail `Error: Check timed out after 60.0s`. No report is ever returned, so neither the ascending-Timestamp ordering of MetricDatapoints nor the presence of the ingested contributor key can be observed. Hypothesis: nothing in the emulator joins ingested log events to an insight rule, and the read side cannot answer regardless — GetInsightRuleReport is still an unmodified scaffold answering HTTP 501 NotImplemented, as already filed in 20261025T053543Z-cloudwatch-getinsightrulereport-stub.md. That open issue does NOT carry this check: its Issue-Trace-Ids name only cloudwatch.deep-cloudwatch-3-cwinsightrank-sum-and-maximum-orderings-disagree, cloudwatch.get-insight-rule-report-returns-metrics-for-active-rule, cloudwatch.insight-rule-contributors-tie-break-order-deterministic and cloudwatch.insight-rule-report-time-window, and implementing the stub alone does not satisfy this check, which additionally requires that rule evaluation over real ingested log data populate Contributors and an ordered MetricDatapoints series. Note also that this check fails by exhausting wall clock rather than by a refusal, because its own convergence loop waits for one Period (60 s) before it issues its first GetInsightRuleReport call.

Why: a client using Contributor Insights to find top talkers cannot do so. Code that polls GetInsightRuleReport until Contributors is non-empty — the documented way to wait out rule-evaluation latency — never terminates against the emulator instead of returning the contributor it just generated traffic for; and any consumer that relies on MetricDatapoints being ascending and duplicate-free by Timestamp has nothing to read.

Fix: the CloudWatch Contributor Insights components — the GetInsightRuleReport handler (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/get_insight_rule_report.rs, still a scaffold) and the insight-rule evaluation state that must aggregate ingested CloudWatch Logs events for an ENABLED CloudWatchLogRule into contributors and per-Period datapoints, so a report over a window of at least one Period returns them in ascending Timestamp order; keep the existing INVALID_TIME_RANGE InvalidParameterValue refusal for windows shorter than one Period.

Traces: cloudwatch.get-insight-rule-report-time-series-ordering.

## Evidence
Replay verdict for cloudwatch.get-insight-rule-report-time-series-ordering: [mismatch] `Error: Check timed out after 60.0s`.

Expected (recorded AWS behavior): after PutInsightRule(RuleName=<rule>, RuleDefinition={Schema:{Name:CloudWatchLogRule,Version:1}, LogGroupNames:[<group>], LogFormat:JSON, Contribution:{Keys:["$.ip"],Filters:[]}, AggregateOn:Count}, RuleState=ENABLED) and repeated PutLogEvents of {"ip":"203.0.113.42",...}, a GetInsightRuleReport(RuleName=<rule>, StartTime, EndTime, Period=60, Metrics=["UniqueContributors"]) over a window of at least one Period returns Contributors containing Keys == ("203.0.113.42",) and a MetricDatapoints list whose Timestamps are strictly ascending with no duplicates.

Actual: all setup calls are accepted, but no report with a non-empty Contributors list is ever produced; the check exhausts its budget with no successful GetInsightRuleReport response, so the ordering and contributor assertions are never reached. GetInsightRuleReport is additionally an unimplemented scaffold (HTTP 501 NotImplemented) per the open issue 20261025T053543Z-cloudwatch-getinsightrulereport-stub.md, which does not list this check id.

## Steps to Reproduce
1. Boot the emulator and point a botocore cloudwatch + logs client at it.
2. logs.create_log_group(logGroupName=G); logs.create_log_stream(logGroupName=G, logStreamName=S).
3. cloudwatch.put_insight_rule(RuleName=R, RuleState="ENABLED", RuleDefinition=json.dumps({"Schema":{"Name":"CloudWatchLogRule","Version":1},"LogGroupNames":[G],"LogFormat":"JSON","Contribution":{"Keys":["$.ip"],"Filters":[]},"AggregateOn":"Count"})).
4. Repeatedly logs.put_log_events(logGroupName=G, logStreamName=S, logEvents=[{"timestamp": now_ms, "message": json.dumps({"ip":"203.0.113.42","requestBytes":128})} x3]).
5. Poll cloudwatch.get_insight_rule_report(RuleName=R, StartTime=t0, EndTime=now, Period=60, Metrics=["UniqueContributors"]) once the window spans at least one Period, until Contributors is non-empty.
6. Observe that a report with contributors never arrives (the call itself is refused with HTTP 501 NotImplemented), so the ascending-Timestamp MetricDatapoints assertion and the contributor-key assertion can never be evaluated.
7. Or replay the check id in trace_ids.
