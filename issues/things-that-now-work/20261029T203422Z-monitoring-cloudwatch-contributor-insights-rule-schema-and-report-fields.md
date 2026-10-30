Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatch-custom-insight-rule-lifecycle-with-report
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatch-custom-insight-rule-lifecycle-with-report cwp-describe-insight-rules-pagination monitoring-contributor-insights-and-managed-rules cloudwatch-contributor-insights-rule-lifecycle
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|DescribeInsightRules|2xx->2xx|$.InsightRules[Name="sim2real-probe-cloudwatch-insight-rule-9b57dbc8"].Schema
Issue-Key: monitoring|DescribeInsightRules|2xx->2xx|$.InsightRules[*].Schema
Issue-Key: monitoring|DescribeInsightRules|2xx->2xx|$.InsightRules[Name="sim2real-probe-cwp-irpg-5a2e20f8-a"].Schema
Issue-Key: monitoring|DescribeInsightRules|2xx->2xx|$.InsightRules[Name="sim2real-probe-cwp-irpg-5a2e20f8-b"].Schema
Issue-Key: monitoring|DescribeInsightRules|2xx->2xx|$.InsightRules[Name="sim2real-probe-cloudwatch-insight-rule-0bc25703"].Schema
Issue-Key: monitoring|GetInsightRuleReport|2xx->2xx|$.AggregationStatistic
Issue-Key: monitoring|GetInsightRuleReport|2xx->2xx|$.MetricDatapoints[*]
Issue-Key: monitoring|GetInsightRuleReport|2xx->2xx|$.RuleAttributes
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CloudWatch Contributor Insights rule Schema and report fields are misrendered

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInsightRules

## Description
6 causes, one shared fault: the Contributor Insights read models are serialized from the emulator's internal representation rather than the wire shapes AWS returns — the rule Schema is emitted as a JSON object instead of the service's compact string, and GetInsightRuleReport omits or mislabels the aggregation members.

Cause 1 of 6: DescribeInsightRules rule Schema. AWS (recorded behavior; the model only types it loosely): Smithy InsightRule.Schema targets InsightRuleSchema, a plain string, and the member doc describes the logical value as {"Name": "CloudWatchLogRule", "Version": 1}; the service puts that on the wire as the compact string 'CloudWatchLogRule/1' (and 'ServiceLogRule/1' for managed rules) — fixed text, with the name and version coming from the rule's schema. Emulator: returns the literal JSON object text '{"Name": "CloudWatchLogRule", "Version": 1}'. Why: a client comparing Schema to 'CloudWatchLogRule/1' (or splitting on '/') to tell a customer rule from a managed one never matches, so rule inventories misclassify every rule. Fix: the DescribeInsightRules response serializer / the InsightRule projection in the CloudWatch crate. Trace: cloudwatch-custom-insight-rule-lifecycle-with-report#task#2.

Cause 2 of 6: same Schema defect on the paginated read path, first page. Trace: cwp-describe-insight-rules-pagination#task#3.

Cause 3 of 6: same Schema defect when several rules are returned in one page (every element carries the object form). Trace: cwp-describe-insight-rules-pagination#task#5.

Cause 4 of 6: same Schema defect on the rule created through the mixed custom/managed workflow. Trace: monitoring-contributor-insights-and-managed-rules#task#2.

Cause 5 of 6: GetInsightRuleReport on a rule with no contributor traffic. AWS: AggregationStatistic is the report's aggregation statistic as the service names it — 'SampleCount' for a rule that aggregates on counts; RuleAttributes is present (an empty list when nothing is requested); and MetricDatapoints is populated with one entry per Period in [StartTime, EndTime) carrying Timestamp and the requested metric members (UniqueContributors 0.0 for an idle rule) rather than being empty. Smithy: GetInsightRuleReport output members AggregationStatistic (InsightRuleAggregationStatistic, string), MetricDatapoints (InsightRuleMetricDatapoints). Emulator: AggregationStatistic 'Count', no RuleAttributes member, and an empty MetricDatapoints list. Why: a client charting the report gets no x-axis at all and reads an aggregation statistic that is not one the service emits, so dashboards and the ApproximateUniqueCount/UniqueContributors series are silently empty. Fix: the GetInsightRuleReport handler and its output projection. Trace: cloudwatch-contributor-insights-rule-lifecycle#task#2.

Cause 6 of 6: GetInsightRuleReport on a rule that has been exercised — AggregationStatistic 'Count' instead of 'SampleCount' and the RuleAttributes member absent. Same component as Cause 5 but on the populated path, so fixing the empty-report path alone does not close it. Trace: cloudwatch-custom-insight-rule-lifecycle-with-report#task#6.

Related but NOT covering: open issue 20261029T020236Z-cloudwatch-getinsightrulereport-3-net-new-root-causes-period is a behavioral-corpus issue about Period validation and AggregationStatistic echoing; it carries no wire replay key for these fixtures.

## Evidence
Replay (simparity-wire-aws-v1), 6 clusters.
1. DescribeInsightRules, cloudwatch-custom-insight-rule-lifecycle-with-report#task#2: $.InsightRules[Name="…"].Schema expected 'CloudWatchLogRule/1', got '{"Name": "CloudWatchLogRule", "Version": 1}' (also #task#4).
2. cwp-describe-insight-rules-pagination#task#3: $.InsightRules[0].Schema got '{"Name": "CloudWatchLogRule", "Version": 1}' (also #task#4).
3. cwp-describe-insight-rules-pagination#task#5: both returned rules' Schema expected 'CloudWatchLogRule/1'.
4. monitoring-contributor-insights-and-managed-rules#task#2: Schema expected 'CloudWatchLogRule/1' (also #task#4, #task#6).
5. GetInsightRuleReport, cloudwatch-contributor-insights-rule-lifecycle#task#2: $.AggregationStatistic expected 'SampleCount', got 'Count'; $.RuleAttributes expected [], got None; $.MetricDatapoints[0..12] expected {'Timestamp': …, 'UniqueContributors': 0.0} each, got None.
6. GetInsightRuleReport, cloudwatch-custom-insight-rule-lifecycle-with-report#task#6: $.AggregationStatistic expected 'SampleCount', got 'Count'; $.RuleAttributes expected [], got None (also monitoring-contributor-insights-and-managed-rules#task#7).

## Steps to Reproduce
1. Boot the emulator from mainline and replay the simparity-wire-aws-v1 corpus.
2. PutInsightRule a CloudWatchLogRule-schema rule, then DescribeInsightRules (fixture cloudwatch-custom-insight-rule-lifecycle-with-report step task#2) and read InsightRules[].Schema.
3. Repeat on the paginated path: cwp-describe-insight-rules-pagination steps task#3 and task#5, and monitoring-contributor-insights-and-managed-rules#task#2.
4. Call GetInsightRuleReport over a window spanning several Periods (cloudwatch-contributor-insights-rule-lifecycle#task#2 and cloudwatch-custom-insight-rule-lifecycle-with-report#task#6) and inspect AggregationStatistic, RuleAttributes and MetricDatapoints.
