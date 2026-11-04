Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pi.list-available-resource-metrics-includes-db-load pi.tag-resource-reserved-prefix-key-rejected pi.describe-dimension-keys-sorted-by-load-descending
Issue-Shape-Id: com.amazonaws.pi#PerformanceInsightsv20180227
Issue-Key: pi.list-available-resource-metrics-includes-db-load|run#9|AssertionError|assert metric_names, (
Issue-Key: pi.tag-resource-reserved-prefix-key-rejected|run#8|Timeout|_time.sleep(2)
Issue-Key: pi.describe-dimension-keys-sorted-by-load-descending|_lock_contention#20|OperationalError|setup = pymysql.connect(
Issue-Commit: 2a533320acecc5e3d5510f43fe45b8adcca9ff2d

# pi: 3 net-new causes — the `db` metric catalog is empty, the PI-enabled instance endpoint accepts no MySQL session, and DescribeDimensionKeys always reports zero keys

> _Note: this issue was identified against the code base at commit `2a533320acecc5e3d5510f43fe45b8adcca9ff2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAvailableResourceMetrics

## Description
All three failing `pi` behavioral checks come from three distinct root causes in the Performance Insights emulator (and in the RDS endpoint it reports on). Three causes to close.

Cause 1 of 3: ListAvailableResourceMetrics answers an EMPTY catalog for MetricTypes=["db"].
AWS: Smithy `ListAvailableResourceMetricsRequest$MetricTypes` documents the valid types as `os` (OS counter metrics) - All engines; `db` (DB load metrics) - All engines except for Amazon DocumentDB; `db.sql.stats`; `db.sql_tokenized.stats`. Output member is `Metrics` (ResponseResourceMetricList). Recorded behavior: for a PI-enabled MySQL RDS instance (EnablePerformanceInsights=True, DatabaseInsightsMode=advanced), ServiceType=RDS + MetricTypes=["db"] returns HTTP 200 with a NON-EMPTY `Metrics` list of the engine's native `db.*` counters, and that list contains no `db.load*` entry (the Active Session History load series is reached through GetResourceMetrics / ListAvailableResourceDimensions, not through this catalog).
Emulator: returns HTTP 200 with `Metrics: []` for every data source — the `db` type's catalog is a hard-coded empty slice, so no counter is ever advertised. The check polls to stability for 180 s and still sees an empty set, failing on `ListAvailableResourceMetrics(MetricTypes=['db']) returned no counters at all`. The same empty answer is what makes `pi.tag-resource-reserved-prefix-key-rejected` fail: its positive-control loop polls this operation until `Metrics` is non-empty, never converges, and the check is killed by the harness at the 60 s cap (reported as `Check timed out after 60.0s`).
Why: a client enumerates the counters it may query from this catalog. An empty list reads as "this instance publishes no DB metrics", so dashboard/alarm wiring built from the catalog builds nothing, and any readiness gate that waits for the catalog to populate waits forever instead of proceeding.
Fix: the ListAvailableResourceMetrics handler's catalog data for the `db` metric type (the `db` entry of the published-metric-types table currently points at an empty counter list), plus whatever state the service keeps per data source so the catalog can be scoped to the instance's engine family. Note that closing this also unblocks `pi.tag-resource-reserved-prefix-key-rejected`, whose actual subject — TagResource against a well-formed but nonexistent pi analysis-report ARN must answer `NotAuthorizedException` with the message `The specified resource is not authorized for this account` (fixed text, no interpolated values), and a ResourceARN outside the modelled `^arn:.*:pi:.*$` pattern must answer `ValidationException` with `Validation error(s) detected: Value at 'resourceARN' failed to satisfy constraints.` (fixed text; `resourceARN` is the member name from the model) — is still UNGRADED, because the check never reached those two calls. Both refusals must be verified when re-replaying that check.

Cause 2 of 3: the endpoint the emulator publishes for a PI-enabled MySQL instance accepts no MySQL connection, so no DB load can be driven.
AWS: recorded behavior only. `DescribeDBInstances` returns `Endpoint.Address`/`Endpoint.Port` for the created MySQL instance, and a client in the instance's VPC opens a MySQL session on that address; the check uses the session to create row-lock contention and a CPU-bound BENCHMARK() so that the instance reports more than one differently weighted `db.wait_event` dimension key.
Emulator: the address it publishes refuses the TCP/MySQL connection outright — `pymysql.err.OperationalError: (2003, "Can't connect to MySQL server on '127.0.0.1' ([Errno 111] Connection refused)")`. The emulator announced a DB endpoint that nothing serves, so the check errors before any pi call under test is made.
Why: every PI contract that depends on real session activity (dimension keys, per-SQL metrics, wait-event ordering) is unreachable; a client cannot distinguish "idle instance" from "no engine at all".
Fix: the RDS DB-instance data plane behind the published instance endpoint (the MySQL wire listener the endpoint address/port resolves to) and the endpoint the RDS describe path reports. This root cause is also carried for the rds service by the open issues `human-rds-the-published-db-instance-endpoint-refuses-every-engine.md` and `20261104T053605Z-rds-the-published-db-instance-cluster-endpoint-serves.md`; neither carries this pi check's replay key, so it is filed here for the pi check and should be closed together with them.

Cause 3 of 3: DescribeDimensionKeys reports no dimension keys for any window.
AWS: Smithy `DescribeDimensionKeys` output carries `Keys` (DimensionKeyDescriptionList), each entry with `Dimensions` and a float `Total`. Recorded behavior: with `Metric=db.load.avg` and `GroupBy={Group: db.wait_event, Limit: 10}` over a 15-minute window on a loaded instance, `Keys` holds the top-N wait-event groups, each with its `Dimensions` map and a float `Total`, ordered by `Total` DESCENDING.
Emulator: the handler reports an empty `Keys` list (and an empty `PartitionKeys`) unconditionally — it holds no sampled sessions for any data source, so no window can ever produce two differently weighted keys and the descending-`Total` ordering contract is unsatisfiable even once cause 2 is fixed.
Why: a client that reads top-N load contributors from PI gets "no load from anything" for a busy instance, and ordering-dependent consumers (top-waits dashboards, triage automation) have nothing to rank.
Fix: the DescribeDimensionKeys handler and the service state it reads — it needs a per-data-source session/wait-event sample set that the window aggregation can group and sort by `Total` descending, consistent with what GetResourceMetrics reports for `db.load.avg`.

Traces: see trace_ids — `pi.list-available-resource-metrics-includes-db-load` (cause 1), `pi.tag-resource-reserved-prefix-key-rejected` (cause 1, times out in its positive-control poll; its TagResource refusals remain ungraded), `pi.describe-dimension-keys-sorted-by-load-descending` (causes 2 and 3). Handler citations: `services/service-com-amazonaws-pi-performanceinsightsv20180227/src/ops/list_available_resource_metrics.rs` lines 379 (`DB_COUNTER_METRICS` empty) and 551-572 (`PUBLISHED_METRIC_TYPES` binding `db` to it); `services/service-com-amazonaws-pi-performanceinsightsv20180227/src/ops/describe_dimension_keys.rs` lines 8-14 (empty `Keys`/`PartitionKeys` by construction).

## Evidence
Suite: pi 16/19 (84%), 3 failed; 2 mismatched, 1 errored.

[mismatch] pi.list-available-resource-metrics-includes-db-load
  AssertionError: ListAvailableResourceMetrics(MetricTypes=['db']) returned no counters at all
  Expected (AWS): HTTP 200, Metrics = non-empty list of the MySQL engine's native db.* counters, none of them matching db.load*.
  Actual (emulator): HTTP 200, Metrics = [] on every poll for 180 s.

[mismatch] pi.tag-resource-reserved-prefix-key-rejected
  Error: Check timed out after 60.0s
  Cause: the check's positive control polls pi.list_available_resource_metrics(ServiceType='RDS', Identifier=<dbi>, MetricTypes=['db']) until Metrics is non-empty; the emulator always returns Metrics=[], so the loop never exits.
  Consequently UNGRADED: TagResource(ResourceARN=<well-formed nonexistent pi report ARN>, Tags=[{Key:'aws:reserved'}]) must be NotAuthorizedException / "The specified resource is not authorized for this account"; TagResource(ResourceARN='not-an-arn-at-all') must be ValidationException / "Validation error(s) detected: Value at 'resourceARN' failed to satisfy constraints."

[error] pi.describe-dimension-keys-sorted-by-load-descending
  pymysql.err.OperationalError: (2003, "Can't connect to MySQL server on '127.0.0.1' ([Errno 111] Connection refused)")
  Expected (AWS): the Endpoint.Address/Port returned by DescribeDBInstances accepts a MySQL session; after concurrent row-lock + CPU load, DescribeDimensionKeys(Metric='db.load.avg', GroupBy={Group:'db.wait_event', Limit:10}) returns >=2 Keys, each with Dimensions and a float Total, with Totals sorted descending.
  Actual (emulator): the published endpoint refuses the connection, so no load is generated; the DescribeDimensionKeys handler reports Keys=[] for every window regardless.

## Steps to Reproduce
1. Create a PI-enabled MySQL RDS instance: rds.create_db_instance(Engine='mysql', DBInstanceClass='db.t3.medium', AllocatedStorage=20, EnablePerformanceInsights=True, DatabaseInsightsMode='advanced', PerformanceInsightsRetentionPeriod=465) in a subnet group, and wait for DBInstanceStatus='available'. Take DbiResourceId from describe_db_instances.
2. Cause 1: call pi.list_available_resource_metrics(ServiceType='RDS', Identifier=<DbiResourceId>, MetricTypes=['db']). Observe Metrics == [] (AWS: a non-empty list of db.* engine counters, with no db.load* entry).
3. Cause 1 (second symptom): run pi.tag-resource-reserved-prefix-key-rejected — its positive-control poll on step 2 never sees a non-empty Metrics list, so the check is killed at the 60 s harness cap and neither TagResource refusal is exercised.
4. Cause 2: read Endpoint.Address/Endpoint.Port from describe_db_instances for the same instance and open a MySQL connection to it (e.g. pymysql.connect(host=<Address>, port=<Port>, user='piadmin', password=<master password>)). Observe ECONNREFUSED.
5. Cause 3: call pi.describe_dimension_keys(ServiceType='RDS', Identifier=<DbiResourceId>, StartTime=now-15min, EndTime=now, Metric='db.load.avg', GroupBy={'Group':'db.wait_event','Limit':10}) for any window. Observe Keys == [] (AWS: top-N wait-event keys, each with Dimensions and a float Total, ordered by Total descending).
