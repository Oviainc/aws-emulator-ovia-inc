Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatch-contributor-insights-rule-lifecycle
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatch-contributor-insights-rule-lifecycle cloudwatch-custom-insight-rule-lifecycle-with-report monitoring-contributor-insights-and-managed-rules
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|GetInsightRuleReport|2xx->2xx|$.RuleAttributes
Issue-Commit: 2c3c98693e0f0f7218b860bf5e220437e88b6cdd

# CloudWatch GetInsightRuleReport omits the RuleAttributes member

> _Note: this issue was identified against the code base at commit `2c3c98693e0f0f7218b860bf5e220437e88b6cdd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInsightRuleReport

## Description
1 cause: the GetInsightRuleReport success response is assembled from the emulator's internal report representation, and the RuleAttributes member of the output structure is never emitted at all.

Cause 1 of 1: GetInsightRuleReport on an existing Contributor Insights rule, over any window, with no rule attributes requested.

AWS: the 200 response always carries the RuleAttributes member of GetInsightRuleReportOutput. When the request asks for no contributor/rule attributes (no attribute entries in Metrics), the member is present and is an EMPTY list, not absent — an empty collection is still serialized on this path. This is recorded behavior only for the repo's model snapshot: the on-disk Smithy model (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801, com.amazonaws.cloudwatch#GetInsightRuleReportOutput) types only KeyLabels, AggregationStatistic, AggregateValue, ApproximateUniqueCount, Contributors and MetricDatapoints, so RuleAttributes is not modeled there and the requirement comes from the recorded 200 responses, which show `RuleAttributes: []`. There is no status, error code or error message involved — this is a success-path shape divergence (HTTP 200 both sides).

Emulator: answers 200 with the member entirely absent (replay reports `$.RuleAttributes: missing (expected [], got None)`), on both the idle-rule path and the exercised-rule path.

Why: a client that reads the attribute series unconditionally — `response['RuleAttributes']` in boto3, or an SDK shape whose field is populated from the wire — sees a missing key rather than an empty list, so code that iterates or lengths the attribute list raises instead of producing a zero-attribute report. Any consumer that distinguishes "the service returned no attributes" from "the service did not answer this member" cannot do so against the emulator.

Fix: the GetInsightRuleReport output projection/serializer in the CloudWatch service crate — the member must be emitted (empty list when no attributes are requested) for every 200 report, on both the no-traffic and the populated report paths, since the same projection serves both.

Traces: cloudwatch-contributor-insights-rule-lifecycle#task#2 (representative); same cluster also at cloudwatch-custom-insight-rule-lifecycle-with-report#task#6 and monitoring-contributor-insights-and-managed-rules#task#7.

Related but NOT covering: the RESOLVED issue 20261029T203422Z-monitoring-cloudwatch-contributor-insights-rule-schema-and-report-fields.md bundled this member with AggregationStatistic and MetricDatapoints; those two now replay clean while RuleAttributes still diverges, and a resolved issue covers nothing. The open issue 20261029T020236Z-cloudwatch-getinsightrulereport-3-net-new-root-causes-period is a behavioral-corpus issue about Period validation and AggregationStatistic echoing and carries no wire replay key for this step; the open issue 20261029T203422Z-monitoring-cloudwatch-contributor-insights-error-paths-use-emulator-authored.md covers only GetInsightRuleReport refusal paths (unknown rule name, token/MaxResults validation), not this 200-response member.

## Evidence
Replay (simparity-wire-aws-v1), 1 cluster, 3 affected steps, verdict mismatch.
GetInsightRuleReport, cloudwatch-contributor-insights-rule-lifecycle#task#2: status expected 200, got 200; `$.RuleAttributes: missing (expected [], got None)`.
Divergence key: service=monitoring|GetInsightRuleReport|status=200->200|keys=$.RuleAttributes.
Affected: cloudwatch-contributor-insights-rule-lifecycle#task#2, cloudwatch-custom-insight-rule-lifecycle-with-report#task#6, monitoring-contributor-insights-and-managed-rules#task#7.
No stub blocks this cluster (stub_path empty) and no open issue owns it (owned_by empty), i.e. it is the only unowned cluster in the 65-cluster replay result.
Model check: smithy_query on the repo's cloudwatch model shows GetInsightRuleReportOutput without a RuleAttributes member, so the member must be added to the output shape/serializer as well as populated.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the simparity-wire-aws-v1 wire corpus.
2. PutInsightRule a CloudWatchLogRule-schema Contributor Insights rule (fixture cloudwatch-contributor-insights-rule-lifecycle).
3. Call GetInsightRuleReport for that rule over a window of several Periods with no rule/contributor attributes requested — step task#2 of that fixture.
4. Inspect the 200 body: AWS returns `RuleAttributes: []`; the emulator omits the member entirely.
5. Repeat on the exercised-rule path (cloudwatch-custom-insight-rule-lifecycle-with-report#task#6 and monitoring-contributor-insights-and-managed-rules#task#7) — the member is absent there too, so both report paths must be confirmed after the fix.
