Issue-Source: trace-sweep-divergence
Issue-Trace: monitoring-custom-metrics-publish-and-query
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: monitoring-custom-metrics-publish-and-query cwm-dimension-count-and-empty-value cwm-unit-out-of-enum cwm-entity-strict-missing-name-400 cwm-value-magnitude-and-infinity cwm-values-counts-length-mismatch cwm-namespace-leading-colon-accepted-aws-prefix-rejected cwm-statistic-set-consistency cwp-get-metric-data-query-id-rejections cwp-get-metric-data-query-shape-rejections-and-messages
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|GetMetricStatistics|2xx->2xx|$.Datapoints
Issue-Key: monitoring|GetMetricData|2xx->2xx|$.MetricDataResults[*].Timestamps
Issue-Key: monitoring|GetMetricData|2xx->2xx|$.MetricDataResults[*].Values
Issue-Key: monitoring|PutMetricData|4xx->4xx|$.message
Issue-Key: monitoring|PutMetricData|2xx->4xx|status
Issue-Key: monitoring|PutMetricData|4xx->2xx|$
Issue-Key: monitoring|PutMetricData|4xx->2xx|$headers.x-amzn-query-error
Issue-Key: monitoring|PutMetricData|4xx->2xx|status
Issue-Key: monitoring|GetMetricData|4xx->2xx|$.__type
Issue-Key: monitoring|GetMetricData|4xx->2xx|$.message
Issue-Key: monitoring|GetMetricData|4xx->2xx|$headers.x-amzn-query-error
Issue-Key: monitoring|GetMetricData|4xx->2xx|status
Issue-Key: monitoring|GetMetricData|4xx->4xx|$.__type
Issue-Key: monitoring|GetMetricData|4xx->4xx|$.message
Issue-Key: monitoring|GetMetricData|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: monitoring|GetMetricData|2xx->2xx|$.MetricDataResults[*].Label
Issue-Related: 20261029T203422Z-monitoring-cloudwatch-error-envelope-omits-x-amzn-query-error.md
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CloudWatch published metric data is unqueryable and metric-query validation diverges

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
9 causes in the metric ingest/query path: data accepted by PutMetricData is not retrievable, PutMetricData's validation surface is wrong in both directions, and GetMetricData's query validation and labelling diverge.

Cause 1 of 9: GetMetricStatistics over a window into which PutMetricData has just published. AWS: 200 with one Datapoint per Period that has data, carrying the requested statistics (SampleCount/Sum/Minimum/Maximum/Average as requested), the period's Timestamp and the Unit the data was published with. Smithy: GetMetricStatistics output Datapoints targets Datapoints. Emulator: 200 with Datapoints: []. Why: the whole custom-metric workflow is broken — a client publishes successfully and can never read the data back, so no chart, no capacity report and no alarm baseline works. Fix: the metric store's aggregation/read path behind GetMetricStatistics (the published datapoints are not being indexed into the queried namespace/metric/dimension/period bucket). Trace: monitoring-custom-metrics-publish-and-query#task#10.

Cause 2 of 9: GetMetricData over the same window. AWS: 200 and MetricDataResults[0].Timestamps/Values carry the aggregated series (number of timestamps always equals number of values). Emulator: both arrays empty. Same store, different read path, so it does not close with Cause 1 alone. Trace: monitoring-custom-metrics-publish-and-query#task#11.

Cause 3 of 9: PutMetricData with a Dimension whose Value is the empty string. AWS: 400 with __type 'com.amazon.coral.service#MissingRequiredParameterException', header 'x-amzn-query-error: MissingParameter;Sender' and the message 'The parameter MetricData.member.<n>.Dimensions.member.<m>.Value is required.' — fixed text with the query-style 1-based member path of the offending dimension. Smithy: PutMetricData models MissingRequiredParameterException (awsQueryError MissingParameter). Emulator: 400 with __type 'ValidationException', no header, and the generic "1 validation error detected: Value '' at 'metricData.1.member.dimensions.1.member.value' failed to satisfy constraint: Member must have length greater than or equal to 1". Why: an empty dimension value is a MISSING parameter to AWS, not a length violation; the error code and the member path form both differ, so a caller cannot map the failure back to the submitted datum. Fix: PutMetricData's dimension validation and its member-path rendering. Trace: cwm-dimension-count-and-empty-value#task#3.

Cause 4 of 9: PutMetricData with EntityMetricData under StrictEntityValidation where the entity's KeyAttributes omit a required key. AWS: 400 with the message 'MissingRequiredFields: Missing required key attribute <AttributeName> for entity of type <Type>' — fixed prefix plus the specific missing attribute name and the entity Type from the request. Emulator: the generic 'MissingRequiredFields: There are missing required fields in the KeyAttributes for the provided Type.' Why: the caller is told that something is missing but not WHICH key attribute, so a strict-entity publish cannot be repaired from the error. Fix: PutMetricData's entity KeyAttributes validation message. Trace: cwm-entity-strict-missing-name-400#task#2.

Cause 5 of 9: PutMetricData whose Namespace begins with ':'. AWS (recorded behavior only; the model's PutMetricData.Namespace @pattern '^[^:]' is NOT enforced this way by the service): 200 — the namespace is accepted; only the reserved 'AWS/' prefix is refused. Emulator: 400. Why: a client using a leading-colon namespace is refused data it can publish against AWS, so the publish path is unusable for those namespaces. Fix: PutMetricData's namespace validation (reject only the reserved AWS/ prefix; do not apply the model pattern as a hard constraint on this member). Trace: cwm-namespace-leading-colon-accepted-aws-prefix-rejected#task#1.

Cause 6 of 9: PutMetricData with a StatisticValues set whose Maximum is below its Minimum. AWS: 400 with __type 'com.amazon.coral.service#InvalidParameterValueException', header 'InvalidParameterValue;Sender' and the message 'The parameter MetricData.member.<n>.StatisticValues.Maximum must be greater than MetricData.member.<n>.StatisticValues.Minimum.' — fixed text with the query-style member index. Emulator: 200 with an empty body — the inconsistent statistic set is STORED. Why: a corrupt statistic set enters the metric store and poisons every subsequent Average/Min/Max read of that metric. Fix: PutMetricData's StatisticSet consistency check. Trace: cwm-statistic-set-consistency#task#2.

Cause 7 of 9: GetMetricData whose MetricDataQueries[].Id does not match the service's query-id grammar. AWS: 400 with __type 'com.amazon.coral.validate#ValidationException', header 'ValidationError;Sender' and the message 'The value for parameter MetricDataQueries.member.<n>.Id is not matching the expected pattern ^[a-z][a-zA-Z0-9_]*$.' followed by a newline — fixed text with the member index. Emulator: 200 — the query runs. Why: a client with an illegal query id (e.g. starting with a capital) gets results from the emulator and a hard refusal from AWS, hiding a real defect until production. Fix: GetMetricData's query-id validation. Trace: cwp-get-metric-data-query-id-rejections#task#3.

Cause 8 of 9: GetMetricData with two queries sharing one Id. AWS: 400 with __type 'com.amazon.coral.validate#ValidationException', header 'ValidationError;Sender' and the message 'The values for parameter id in MetricDataQueries are not unique.' followed by a newline — entirely fixed text. Emulator: 400 but __type 'InvalidParameterValueException', header 'InvalidParameterValue;Sender' and its own prose 'the Id m1 is used by more than one query; each query carries its own Id'. Why: the error class flips from request validation to parameter value and the text is invented, so code branching on either diverges. Fix: GetMetricData's duplicate-id check (classify as ValidationException) and its message. Trace: cwp-get-metric-data-query-id-rejections#task#5.

Cause 9 of 9: GetMetricData for an Expression query with no explicit Label. AWS: MetricDataResults[].Label defaults to the query's own Id for an expression result. Smithy: MetricDataResult.Label is 'The human-readable label associated with the data'. Emulator: returns the expression source text itself as the Label. Why: a client keying chart series or result lookup on Label gets the expression string instead of the id it supplied, so results cannot be correlated to queries. Fix: GetMetricData's result labelling for expression queries. Trace: cwp-get-metric-data-query-shape-rejections-and-messages#task#1.

Note on values: the recorded datapoint numbers and epoch timestamps are that run's data, not requirements — the requirement is that published data is returned over its window with matching timestamp/value cardinality.

## Evidence
Replay (simparity-wire-aws-v1), 9 clusters.
1. GetMetricStatistics, monitoring-custom-metrics-publish-and-query#task#10: $.Datapoints expected one entry with Maximum/Minimum/SampleCount/Sum/Timestamp/Unit, got [].
2. GetMetricData, monitoring-custom-metrics-publish-and-query#task#11: MetricDataResults[0].Timestamps expected [<ts>], got []; .Values expected [<sum>], got [].
3. PutMetricData, cwm-dimension-count-and-empty-value#task#3: __type expected 'com.amazon.coral.service#MissingRequiredParameterException', got 'ValidationException'; message expected 'The parameter MetricData.member.1.Dimensions.member.1.Value is required.', got "1 validation error detected: Value '' at 'metricData.1.member.dimensions.1.member.value' …"; header 'MissingParameter;Sender' missing. (also cwm-unit-out-of-enum#task#2)
4. PutMetricData, cwm-entity-strict-missing-name-400#task#2: message expected 'MissingRequiredFields: Missing required key attribute Name for entity of type Service', got 'MissingRequiredFields: There are missing required fields in the KeyAttributes for the provided Type.' 3 affected steps.
5. PutMetricData, cwm-namespace-leading-colon-accepted-aws-prefix-rejected#task#1: status expected 200, got 400.
6. PutMetricData, cwm-statistic-set-consistency#task#2: status expected 400, got 200; body empty (expected __type 'com.amazon.coral.service#InvalidParameterValueException', message 'The parameter MetricData.member.1.StatisticValues.Maximum must be greater than MetricData.member.1.StatisticValues.Minimum.'); header missing. (also #task#3)
7. GetMetricData, cwp-get-metric-data-query-id-rejections#task#3: status expected 400, got 200; __type/message/header all absent (expected ValidationException + 'The value for parameter MetricDataQueries.member.1.Id is not matching the expected pattern ^[a-z][a-zA-Z0-9_]*$.\n'). (also #task#4)
8. GetMetricData, cwp-get-metric-data-query-id-rejections#task#5: __type expected 'com.amazon.coral.validate#ValidationException', got 'InvalidParameterValueException'; message expected 'The values for parameter id in MetricDataQueries are not unique.\n', got 'the Id m1 is used by more than one query; each query carries its own Id'; header expected 'ValidationError;Sender', got 'InvalidParameterValue;Sender'. 4 affected steps.
9. GetMetricData, cwp-get-metric-data-query-shape-rejections-and-messages#task#1: MetricDataResults[0].Label expected 'e1', got 'm1*2'. (also #task#2)

## Steps to Reproduce
1. Boot the emulator from mainline and replay the simparity-wire-aws-v1 corpus.
2. PutMetricData a StatisticSet into a custom namespace, then GetMetricStatistics (monitoring-custom-metrics-publish-and-query#task#10) and GetMetricData (task#11) over the publish window — observe empty results.
3. PutMetricData with an empty Dimension value (cwm-dimension-count-and-empty-value#task#3), with StrictEntityValidation and an entity missing a key attribute (cwm-entity-strict-missing-name-400#task#2), with a namespace starting ':' (cwm-namespace-leading-colon-accepted-aws-prefix-rejected#task#1), and with StatisticValues Maximum < Minimum (cwm-statistic-set-consistency#task#2).
4. GetMetricData with an Id violating ^[a-z][a-zA-Z0-9_]*$ (cwp-get-metric-data-query-id-rejections#task#3), with two queries sharing an Id (task#5), and with an unlabelled Expression query (cwp-get-metric-data-query-shape-rejections-and-messages#task#1).
