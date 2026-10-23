Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pi.get-resource-metrics-metric-without-aggregate-suffix-rejected pi.get-resource-metrics-returns-full-datapoint-series pi.missing-permission-get-resource-metrics-denied pi.describe-dimension-keys-start-after-end-rejected
Issue-Shape-Id: com.amazonaws.pi#PerformanceInsightsv20180227
Issue-Key: pi.get-resource-metrics-metric-without-aggregate-suffix-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: pi.get-resource-metrics-returns-full-datapoint-series|_poll_until_datapoints#4|Timeout|time.sleep(20)
Issue-Key: pi.missing-permission-get-resource-metrics-denied|_wait_for_denial#12|AssertionError|raise AssertionError(f"expected the call to be denied, but it SUCCEEDED and returned {result!r}")
Issue-Key: pi.describe-dimension-keys-start-after-end-rejected|run#7|AssertionError|assert code == "InvalidArgumentException", (
Issue-Commit: 41eca1b5094053f7eb75d5bb9c5bde3b8db45df6

# PI data plane: GetResourceMetrics accepts unsuffixed metrics, returns an empty series instead of the aligned-window grid, and is served to callers with no IAM permission; DescribeDimensionKeys is a 501 stub (4 causes)

> _Note: this issue was identified against the code base at commit `41eca1b5094053f7eb75d5bb9c5bde3b8db45df6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourceMetrics

## Description
Four distinct root causes in the Performance Insights data plane, all exercised against the shared PI-enabled RDS instance (DbiResourceId db-...). Each cause must be closed before this issue is resubmitted.

Cause 1 of 4: GetResourceMetrics does not require the aggregate-function suffix on a metric name.
AWS: Smithy GetResourceMetrics.MetricQueries (MetricQueryList, len 1..15, required) documents on the member itself: "Each query must specify a Performance Insights metric and specify an aggregate function... You must append the aggregate function to the metric. For example, to find the average for the metric db.load you must use db.load.avg. Valid values for aggregate functions include .avg, .min, .max, and .sum." A metric with no aggregate suffix is refused with the modeled InvalidArgumentException, HTTP 400 (recorded message example: 'The specified statistic is not a known statistic'). The requirement is the 400 + InvalidArgumentException; the message text is an example.
Emulator: MetricQueries=[{"Metric": "db.load"}] over a 30-minute window is ACCEPTED and answered HTTP 200 with MetricList=[{Key:{Metric:'db.load'}, DataPoints:[]}].
Why: a client that mistypes or omits the aggregate function gets an authoritative-looking success with a metric key AWS would never return, so a bug that AWS surfaces at the first call is only discovered in production.
Fix: input validation in the GetResourceMetrics handler — reject a MetricQuery whose Metric lacks one of the documented .avg/.min/.max/.sum suffixes with the modeled InvalidArgumentException before the window arithmetic runs.
Traces: pi.get-resource-metrics-metric-without-aggregate-suffix-rejected.

Cause 2 of 4: GetResourceMetrics returns an empty DataPoints list instead of one point per period across the aligned window.
AWS: recorded behavior only (the model types MetricList as MetricKeyDataPointsList and says nothing about cardinality). The recordings show GetResourceMetrics always returning one DataPoint per PeriodInSeconds period across [AlignedStartTime, AlignedEndTime), each point carrying a Timestamp, with Value present for every period PI has a sample for. The derived requirement is len(DataPoints) == (AlignedEndTime - AlignedStartTime) / PeriodInSeconds, as computed from the target's OWN reported aligned window, and at least one point carrying a Value on a live PI-enabled instance.
Emulator: DataPoints is ALWAYS an empty array. Recorded 200 responses from sibling checks show AlignedStartTime=…15:41, AlignedEndTime=…16:12, PeriodInSeconds=60 (31 periods expected) with MetricList=[{Key:{Metric:'db.load.avg'}, DataPoints:[]}]. The check polls for a Value and therefore never terminates (it was cut off by the harness timeout); the empty grid is the cause, and the handler documents the choice in its module header and at the metric_list construction.
Why: any client that charts, aggregates or waits on a PI series sees zero points and cannot distinguish "no telemetry yet" from "window is wrong"; a dashboard or alarm built on the grid has no x-axis at all. Emitting the Timestamp grid (even with Value omitted where there is no sample, which AWS itself permits) is observable without inventing counters.
Fix: the GetResourceMetrics handler's response construction — materialize one DataPoint per period over the aligned window, honoring PeriodAlignment for whether the stamp is the period start or end.
Traces: pi.get-resource-metrics-returns-full-datapoint-series (and the empty DataPoints visible in the 200s recorded by pi.get-resource-metrics-metric-without-aggregate-suffix-rejected and pi.missing-permission-get-resource-metrics-denied).

Cause 3 of 4: no IAM authorization on the PI data plane — a caller with zero permissions is served.
AWS: recorded behavior only for the code (PI's model declares only InternalServiceError/InvalidArgumentException/NotAuthorizedException as common errors; IAM's implicit deny is not in the model). An IAM user created with no attached policy calling pi:GetResourceMetrics is refused with error code AccessDeniedException. The message AWS returns is built from the request and the caller identity — "User: <user arn> is not authorized to perform: pi:GetResourceMetrics on resource: <identifier> because no identity-based policy allows the pi:GetResourceMetrics action" — so that construction is the requirement; the concrete arn/identifier in any one recording is not.
Emulator: the same call with the unprivileged user's access key SUCCEEDS with HTTP 200 and a normal GetResourceMetricsOutput (recorded: Identifier echoed, MetricList present).
Why: the emulator is over-permissive on authorization, so a client's IAM policy wiring (missing pi:GetResourceMetrics grant, wrong resource scope) is never exercised and ships broken; negative-path tests that assert a denial pass vacuously.
Fix: authorization enforcement for the pi service — gate GetResourceMetrics (and its siblings) on the caller's identity-based policy and return AccessDeniedException for an implicit deny, rather than answering every signed request.
Traces: pi.missing-permission-get-resource-metrics-denied.

Cause 4 of 4: DescribeDimensionKeys is an unimplemented scaffold stub, so it answers 501 NotImplemented — including for requests AWS refuses as argument errors.
AWS: Smithy DescribeDimensionKeys requires ServiceType, Identifier, StartTime, EndTime, Metric and GroupBy, and documents that StartTime must be earlier than EndTime; a reversed window is refused with the modeled InvalidArgumentException, HTTP 400. The successful call returns AlignedStartTime/AlignedEndTime preserving that order plus Keys.
Emulator: DescribeDimensionKeys(StartTime > EndTime) returns error code NotImplemented (HTTP 501). The handler is still the generated stub (its module header carries the 'SCAFFOLDED, NOT IMPLEMENTED' marker and handle() returns Scaffolded::not_implemented).
Why: every DescribeDimensionKeys call fails with a non-AWS error code, so top-N dimension analysis cannot be exercised at all and clients see a 501 where AWS gives a 400 or a result.
Fix: implement the DescribeDimensionKeys handler, including the StartTime<EndTime cross-member check that GetResourceMetrics already performs (the service-wide time-range ordering refusal).
Traces: pi.describe-dimension-keys-start-after-end-rejected. (The second DescribeDimensionKeys check, pi.describe-dimension-keys-sorted-by-load-descending, is blocked upstream by the already-filed EC2 CreateVpc stub and is not part of this issue.)

## Evidence
Cause 1 — pi.get-resource-metrics-metric-without-aggregate-suffix-rejected:
  expected: ClientError code InvalidArgumentException (HTTP 400) for MetricQueries=[{'Metric':'db.load'}]
  actual:   HTTP 200 {'AlignedStartTime': …15:41, 'AlignedEndTime': …16:12, 'Identifier': 'db-00000000000000000000000001', 'MetricList': [{'Key': {'Metric': 'db.load'}, 'DataPoints': []}]}
  assertion: "expected the call to be rejected, but it succeeded and returned {...}"

Cause 2 — pi.get-resource-metrics-returns-full-datapoint-series:
  expected: len(DataPoints) == round((AlignedEndTime-AlignedStartTime)/60) with >=1 point carrying a Value for db.load.avg
  actual:   DataPoints == [] on every poll (same empty list recorded in the 200s of the other two GetResourceMetrics checks, e.g. MetricList=[{'Key': {'Metric': 'db.load.avg'}, 'DataPoints': []}] for aligned window 15:41→16:12 at PeriodInSeconds=60, i.e. 31 periods expected, 0 returned); the polling check never saw a Value and was cut off by the harness timeout.
  source: services/service-com-amazonaws-pi-performanceinsightsv20180227/src/ops/get_resource_metrics.rs:326-338 builds every MetricKeyDataPoints with data_points: Some(Vec::new()); module header lines 8-13 state the series is returned EMPTY.

Cause 3 — pi.missing-permission-get-resource-metrics-denied:
  expected: ClientError code AccessDeniedException, message of the form 'User: <arn> is not authorized to perform: pi:GetResourceMetrics on resource: <identifier> because no identity-based policy allows the pi:GetResourceMetrics action'
  actual:   HTTP 200 {'AlignedStartTime': …15:41, 'AlignedEndTime': …16:12, 'Identifier': 'db-00000000000000000000000001', 'MetricList': [{'Key': {'Metric': 'db.load.avg'}, 'DataPoints': []}]}
  assertion: "expected the call to be denied, but it SUCCEEDED and returned {...}"

Cause 4 — pi.describe-dimension-keys-start-after-end-rejected:
  expected: InvalidArgumentException for StartTime 2026-10-22T16:16:30Z after EndTime 2026-10-22T15:46:30Z
  actual:   error code 'NotImplemented'
  source: services/service-com-amazonaws-pi-performanceinsightsv20180227/src/ops/describe_dimension_keys.rs:4 'SCAFFOLDED, NOT IMPLEMENTED'. Compare the implemented ordering check in get_resource_metrics.rs:210-218.

## Steps to Reproduce
Preconditions: an RDS instance with EnablePerformanceInsights=true and DatabaseInsightsMode=advanced, available; take its DbiResourceId as DBI.

Cause 1: pi.get_resource_metrics(ServiceType='RDS', Identifier=DBI, MetricQueries=[{'Metric': 'db.load'}], StartTime=now-30m, EndTime=now) -> emulator answers 200; AWS answers 400 InvalidArgumentException.

Cause 2: pi.get_resource_metrics(ServiceType='RDS', Identifier=DBI, MetricQueries=[{'Metric': 'db.load.avg'}], StartTime=now-30m, EndTime=now, PeriodInSeconds=60) -> inspect MetricList[0]['DataPoints']: emulator returns []; AWS returns (AlignedEndTime-AlignedStartTime)/60 points, each with a Timestamp and at least one with a Value.

Cause 3: iam.create_user + iam.create_access_key with NO policy attached; build a pi client from that key and repeat the Cause 2 call -> emulator answers 200; AWS answers AccessDeniedException.

Cause 4: pi.describe_dimension_keys(ServiceType='RDS', Identifier=DBI, StartTime=now, EndTime=now-30m, Metric='db.load.avg', GroupBy={'Group': 'db.sql'}) -> emulator answers 501 NotImplemented; AWS answers 400 InvalidArgumentException. Swapping the times back must then succeed with AlignedStartTime < AlignedEndTime.
