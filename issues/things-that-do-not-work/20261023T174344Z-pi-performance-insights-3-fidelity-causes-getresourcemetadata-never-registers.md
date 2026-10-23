Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pi.get-resource-metadata-reflects-sql-digest-feature pi.get-resource-metrics-aligned-time-bounds-invariant pi.list-available-resource-metrics-includes-db-load
Issue-Shape-Id: com.amazonaws.pi#PerformanceInsightsv20180227
Issue-Key: pi.get-resource-metadata-reflects-sql-digest-feature|run#7|Timeout|time.sleep(15)
Issue-Key: pi.get-resource-metrics-aligned-time-bounds-invariant|run#16|AssertionError|assert resp["AlignedEndTime"] in end_candidates, (
Issue-Key: pi.list-available-resource-metrics-includes-db-load|run#11|AssertionError|assert not db_load_counters, (
Issue-Commit: a91eb31cf888327ed8ad916c03309e3893f84f70

# Performance Insights: 3 fidelity causes — GetResourceMetadata never registers SQL_LEVEL_STATISTICS, GetResourceMetrics derives AlignedEndTime from the request, ListAvailableResourceMetrics enumerates db.load

> _Note: this issue was identified against the code base at commit `a91eb31cf888327ed8ad916c03309e3893f84f70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourceMetadata

## Description
Three independent root causes in the emulator's Performance Insights (pi) service, all reached against the shared PI-enabled RDS instance the pi behavioral checks provision (`_create_pi_source`). The same helper is used by all three checks and completes promptly, so none of these failures is an RDS provisioning artifact. 3 causes, numbered below; each must be closed before this issue is resolved.

=== Cause 1 of 3: GetResourceMetadata never reports the SQL_LEVEL_STATISTICS feature on a PI-enabled instance ===
AWS: GetResourceMetadata(ServiceType=RDS, Identifier=<DbiResourceId>) returns `Features`, a map of feature name -> FeatureMetadata{Status}. Smithy: GetResourceMetadata output member `Features` targets FeatureMetadataMap; FeatureStatus enum = ENABLED | DISABLED | UNSUPPORTED | ENABLED_PENDING_REBOOT | DISABLED_PENDING_REBOOT | UNKNOWN. The set of feature KEYS is not enumerated by the model, so recorded behavior only: a real-AWS replay of this check (full 180 s poll) consistently reports the keys DATABASE_INSIGHTS_ADVANCED_MODE, PERFORMANCE_ANALYSIS_REPORTS and SQL_LEVEL_STATISTICS, with SQL_LEVEL_STATISTICS in ENABLED or ENABLED_PENDING_REBOOT on an instance created with EnablePerformanceInsights=True and DatabaseInsightsMode=advanced, and never reports a SQL_DIGEST_STATISTICS key.
Emulator: SQL_LEVEL_STATISTICS never appears in `Features` for such an instance. The check polls GetResourceMetadata every 15 s for up to 180 s waiting for that key; because the key never arrives, the check consumes its whole polling window and is killed by the harness at the 60 s cap (verdict: mismatch, 'Check timed out after 60.0s'). The two other pi checks below use the identical `_create_pi_source` helper and reach their own assertions well inside 60 s, so the time is being spent in this polling loop, not in RDS setup.
Why: a client that discovers PI capabilities through GetResourceMetadata (deciding whether per-query digest statistics are available before issuing DescribeDimensionKeys with GroupBy db.sql / db.sql_tokenized) concludes the feature is absent on an instance where AWS reports it enabled, and either silently degrades or polls forever waiting for registration that never happens.
Fix: GetResourceMetadata's handler and the state it reads — the RDS instance state that records EnablePerformanceInsights / DatabaseInsightsMode must be projected into the PI feature map (services/service-com-amazonaws-pi-performanceinsightsv20180227/src/ops/get_resource_metadata.rs and the pi service state that derives Features from the RDS instance).
Traces: pi.get-resource-metadata-reflects-sql-digest-feature

=== Cause 2 of 3: GetResourceMetrics AlignedEndTime is computed from the requested EndTime instead of the current time ===
AWS: Smithy documents GetResourceMetricsResponse.AlignedEndTime as 'the end time for the returned metrics, after alignment to a granular boundary (as specified by PeriodInSeconds)', and only constrains it to be >= the user-specified EndTime. The DRIVER of the value is recorded behavior only: two live-AWS replays of this check (with different request/now offsets) show AlignedEndTime equal to the CURRENT wall clock at call time floored to the PeriodInSeconds boundary (measured from the Unix epoch) — once landing strictly earlier than the requested EndTime and once strictly later. PI reports only up to the latest period boundary it actually holds, independent of how far in the past the caller asked it to stop. AlignedStartTime is the requested StartTime floored to the same boundary (the emulator gets this one right).
Emulator: AlignedEndTime is a function of the REQUESTED EndTime. With PeriodInSeconds=300, StartTime=now-37min and EndTime=now-2min (17:35:24Z, now ~17:37:24Z), the emulator returned AlignedEndTime=17:40:00Z — the requested EndTime rounded UP to the next 300 s boundary — where the current-time floor is 17:35:00Z.
Why: a caller that pages a metric window, or that uses AlignedStartTime/AlignedEndTime to label and stitch successive GetResourceMetrics windows, is handed an end boundary in the FUTURE relative to the server clock. Buckets that AWS would never claim to cover are claimed as covered, so gaps are read as real zero-load periods and adjacent windows overlap or double-count when stitched.
Fix: the time-alignment logic in GetResourceMetrics' handler — AlignedStartTime from the request, AlignedEndTime from the server's current time, both floored (never ceilinged) to PeriodInSeconds from the epoch (services/service-com-amazonaws-pi-performanceinsightsv20180227/src/ops/get_resource_metrics.rs).
Traces: pi.get-resource-metrics-aligned-time-bounds-invariant

=== Cause 3 of 3: ListAvailableResourceMetrics(MetricTypes=['db']) enumerates a db.load counter that AWS never lists ===
AWS: Smithy: ListAvailableResourceMetrics input requires ServiceType, Identifier and MetricTypes; output `Metrics` is a list of ResponseResourceMetric{Metric, Description, Unit}. Which counters appear is not modeled, so recorded behavior only: two real-AWS replays against a real DB instance, each bounded by a 180 s catalog-lag retry until two consecutive polls agree, show the MetricTypes=['db'] catalog NEVER containing any 'db.load'-prefixed counter. db.load is nonetheless queryable — GetResourceMetrics(Metric='db.load.avg') on the same instance succeeds — i.e. the primary Active Session History metric is deliberately not enumerated as a discoverable counter.
Emulator: the MetricTypes=['db'] catalog includes 'db.load'. Check assertion: "expected no db.load* counter in the MetricTypes=['db'] catalog, found ['db.load']".
Why: a client that enumerates the catalog to build a metric picker, or that treats catalog membership as the contract for which counters it may query and in what dimension/unit, is offered a counter AWS does not advertise. Code generated or validated against the emulator's catalog (e.g. requesting bare 'db.load' rather than the 'db.load.avg' aggregate form) diverges once pointed at AWS.
Fix: the metric catalog that backs ListAvailableResourceMetrics — remove db.load* from the 'db' metric-type catalog while keeping db.load.avg answerable by GetResourceMetrics (services/service-com-amazonaws-pi-performanceinsightsv20180227/src/ops/list_available_resource_metrics.rs and the catalog data it reads).
Traces: pi.list-available-resource-metrics-includes-db-load

## Evidence
Suite: 16/20 passed; pi 16/20 (4 failed). One of the four (pi.describe-dimension-keys-sorted-by-load-descending) is ALREADY FILED, blocked by the unimplemented ec2 CreateInternetGateway stub, and is excluded here.

Cause 1 — pi.get-resource-metadata-reflects-sql-digest-feature [mismatch]
  Expected: Features contains 'SQL_LEVEL_STATISTICS' with Status in (ENABLED, ENABLED_PENDING_REBOOT) within the 180 s registration-lag window.
  Actual: key never present; the check's poll loop runs to the harness cap — 'Check timed out after 60.0s'.

Cause 2 — pi.get-resource-metrics-aligned-time-bounds-invariant [mismatch]
  Request: ServiceType=RDS, MetricQueries=[{Metric: db.load.avg}], PeriodInSeconds=300, StartTime=now-37min, EndTime=now-2min (=17:35:24.567386+00:00), call observed between 17:37:24.567386+00:00 and 17:37:24.579845+00:00.
  Expected AlignedEndTime: 2026-10-23 17:35:00+00:00 (current time floored to the 300 s boundary).
  Actual AlignedEndTime: 2026-10-23 17:40:00+00:00 (requested EndTime ceilinged to the 300 s boundary).
  AssertionError: "AlignedEndTime 2026-10-23 17:40:00+00:00 must equal the current time ... floored to the nearest 300s boundary ([datetime.datetime(2026, 10, 23, 17, 35, tzinfo=datetime.timezone.utc)]); requested EndTime 2026-10-23 17:35:24.567386+00:00 is not the driver of this value".
  AlignedStartTime asserted earlier in the same check passed, so only the end bound is wrong.

Cause 3 — pi.list-available-resource-metrics-includes-db-load [mismatch]
  Request: ListAvailableResourceMetrics(ServiceType=RDS, Identifier=<DbiResourceId>, MetricTypes=['db']).
  Expected: no counter whose name starts with 'db.load'.
  Actual: catalog contains 'db.load'.
  AssertionError: "expected no db.load* counter in the MetricTypes=['db'] catalog, found ['db.load']".

## Steps to Reproduce
Common setup (all three causes): create an RDS DB instance with EnablePerformanceInsights=True, DatabaseInsightsMode='advanced', PerformanceInsightsRetentionPeriod=465, Engine=mysql, DBInstanceClass=db.t3.medium; wait for db_instance_available; take dbi = DescribeDBInstances(...)['DBInstances'][0]['DbiResourceId'].

Cause 1: poll pi.get_resource_metadata(ServiceType='RDS', Identifier=dbi) every 15 s for up to 180 s. Observe that resp['Features'] never contains 'SQL_LEVEL_STATISTICS' (AWS reports it ENABLED or ENABLED_PENDING_REBOOT). Replay check pi.get-resource-metadata-reflects-sql-digest-feature.

Cause 2: now = utcnow(); call pi.get_resource_metrics(ServiceType='RDS', Identifier=dbi, MetricQueries=[{'Metric': 'db.load.avg'}], StartTime=now-37min, EndTime=now-2min, PeriodInSeconds=300). Compare resp['AlignedEndTime'] against floor(now, 300s): the emulator instead returns ceil(requested EndTime, 300s). Replay check pi.get-resource-metrics-aligned-time-bounds-invariant.

Cause 3: call pi.list_available_resource_metrics(ServiceType='RDS', Identifier=dbi, MetricTypes=['db']) (poll until two consecutive responses agree). Inspect {m['Metric'] for m in resp['Metrics']} for names starting with 'db.load' — the emulator lists 'db.load'. Confirm db.load is still queryable via pi.get_resource_metrics(Metric='db.load.avg'). Replay check pi.list-available-resource-metrics-includes-db-load.
