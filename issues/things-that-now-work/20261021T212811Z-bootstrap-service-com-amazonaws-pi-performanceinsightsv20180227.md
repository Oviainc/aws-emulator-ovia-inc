Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the pi (Performance Insights) emulator

_Filed: 2026-10-21T21:28:11Z_
_Operation: service-com-amazonaws-pi-performanceinsightsv20180227 | Class: stub_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 14 operations in this crate is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding zero state, so no client workflow works at all. This issue defines the FIRST minimal, stateful happy-path slice: the performance-analysis-report lifecycle for a monitored DB resource, plus the one read op PI clients always call. Operations outside this slice stay 501 and are filed separately.

Wire protocol (README.md / OPERATIONS.md): `awsJson1_1`, `POST /` with `X-Amz-Target: PerformanceInsightsv20180227.<OperationName>`, sigv4 signing name `pi`.

### Happy path to implement, in order

1. **CreatePerformanceAnalysisReport** — input `ServiceType` (RDS|DOCDB, required), `Identifier` (required, `^[a-zA-Z0-9-]+$`), `StartTime` (required), `EndTime`, `Tags`. Must MINT and PERSIST a new report keyed by (ServiceType, Identifier, AnalysisReportId) and return `AnalysisReportId` matching the modeled pattern `^report-[0-9a-f]{17}$` (i.e. `report-` + 17 lowercase hex chars). The id must be unique per call and must be the id later ops resolve; a fixed/canned id is a failure.
2. **GetPerformanceAnalysisReport** — input `ServiceType`, `Identifier`, `AnalysisReportId` (all required), optional `TextFormat` (MARKDOWN|PLAIN_TEXT), `AcceptLanguage` (EN_US). Must return the stored `AnalysisReport` with `AnalysisReportId`, `Identifier`, `ServiceType`, `CreateTime`, `StartTime`/`EndTime` echoing what Create persisted (not what this request carried), and `Status` (e.g. `SUCCEEDED`). An unknown or deleted `AnalysisReportId` must fail with `InvalidArgumentException` (HTTP 400), not a hollow 200 with an empty report.
3. **ListPerformanceAnalysisReports** — input `ServiceType`, `Identifier` (required), `MaxResults` (0..25), `NextToken` (1..8192, `^[a-zA-Z0-9_=-]+$`), `ListTags`. Must return `AnalysisReports` (list<AnalysisReportSummary>) computed from the store, filtered to the requested `Identifier`/`ServiceType`: every created-and-not-deleted report appears exactly once with its real id, status and times. Honor `MaxResults` by truncating and returning an opaque `NextToken` that resumes exactly where the page stopped, and omit `NextToken` on the last page. When `ListTags` is true, include the stored `Tags` on each summary.
4. **DeletePerformanceAnalysisReport** — input `ServiceType`, `Identifier`, `AnalysisReportId` (all required). Must remove the report from the store and return an empty success body; a subsequent Get must fail and a subsequent List must no longer include it.
5. **GetResourceMetrics** — input `ServiceType`, `Identifier`, `MetricQueries` (1..15, required), `StartTime`, `EndTime` (required), `PeriodInSeconds`, `MaxResults` (0..25), `NextToken`, `PeriodAlignment` (START_TIME|END_TIME). Must return `Identifier` echoing the requested resource, `AlignedStartTime`/`AlignedEndTime` actually ALIGNED down/up to a multiple of the effective period (defaulting the period from the requested time range as the real service does, and aligning per `PeriodAlignment`), and one `MetricKeyDataPoints` entry per element of `MetricQueries` whose `Key.Metric` is that query's metric and whose `DataPoints` cover the aligned window at exactly the period step (one point per interval, `Timestamp` + `Value`). Values may be synthetic but must be deterministic for a given (Identifier, metric, timestamp) so repeated calls agree, and the number/timestamps of points must be derived from the request window and period — never a fixed canned list.

All five must compute their responses from the shared engine store reached via `crate::state::service_state(&*self.engine)` — no canned responses, no echoing the input back as if stored, no minting an id without persisting it. Required-member absence and the modeled `length`/`pattern`/`range` constraints must produce `InvalidArgumentException` (HTTP 400).

## State Model

In `ComAmazonawsPiPerformanceinsightsv20180227State`:

- `reports: HashMap<(ServiceType, String /*Identifier*/), Vec<AnalysisReportRecord>>` — insertion-ordered per resource.
- `AnalysisReportRecord { analysis_report_id: String /*report-<17 hex>*/, service_type: ServiceType, identifier: String, create_time: DateTime, start_time: DateTime, end_time: Option<DateTime>, status: AnalysisStatus, tags: Vec<Tag>, insights: Vec<Insight> }` — `insights` may start empty; Get returns it as the report's `Insights`.
- `report_seq: u64` — monotonic counter mixed into the 17-hex-char id suffix so ids are unique and pattern-valid.
- `list_tokens: HashMap<String /*NextToken*/, ListCursor{ service_type, identifier, offset: usize }>` — opaque pagination cursors for ListPerformanceAnalysisReports (token text must match `^[a-zA-Z0-9_=-]+$`).
- `tags_by_arn: HashMap<String /*ResourceARN*/, BTreeMap<String,String>>` — written by Create's `Tags` so the later Tag/Untag/ListTagsForResource slice has a home.
- Metric data for GetResourceMetrics is derived, not stored: a pure function `datapoint(identifier, metric, timestamp) -> f64` plus the alignment helper, so no per-call mutation happens on a read.

## Evidence

`services/service-com-amazonaws-pi-performanceinsightsv20180227/OPERATIONS.md` lists all 14 operations with their input/output members and errors: CreatePerformanceAnalysisReport (out `AnalysisReportId`, pattern `^report-[0-9a-f]{17}$`), GetPerformanceAnalysisReport (out `AnalysisReport`), ListPerformanceAnalysisReports (out `AnalysisReports` + `NextToken`, `MaxResults` 0..25, `@paginated`), DeletePerformanceAnalysisReport (empty output), GetResourceMetrics (out `AlignedStartTime`, `AlignedEndTime`, `Identifier`, `MetricList`, `NextToken`; `@paginated`). All three declare `InvalidArgumentException (HTTP 400)`, `NotAuthorizedException (HTTP 400)`, `InternalServiceError (HTTP 500)`.

`grep -l '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-pi-performanceinsightsv20180227/src/ops/` returns all 14 handler files (create_performance_analysis_report.rs, get_performance_analysis_report.rs, list_performance_analysis_reports.rs, delete_performance_analysis_report.rs, get_resource_metrics.rs, get_resource_metadata.rs, describe_dimension_keys.rs, get_dimension_key_details.rs, list_available_resource_metrics.rs, list_available_resource_dimensions.rs, list_performance_analysis_report_recommendations.rs, tag_resource.rs, untag_resource.rs, list_tags_for_resource.rs) — i.e. the entire service answers 501 and persists nothing. No tests exercise any PI behavior beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Point an AWS SDK/CLI `pi` client at the emulator endpoint (awsJson1_1, `X-Amz-Target: PerformanceInsightsv20180227.<Op>`).
2. `aws pi create-performance-analysis-report --service-type RDS --identifier db-ABCDEFG1234567 --start-time <t0> --end-time <t1>` → expect 200 with an `AnalysisReportId` matching `^report-[0-9a-f]{17}$`; today 501 NotImplemented.
3. `aws pi get-performance-analysis-report --service-type RDS --identifier db-ABCDEFG1234567 --analysis-report-id <id from step 2>` → expect the stored report with matching Identifier/ServiceType/StartTime/EndTime and a Status.
4. `aws pi list-performance-analysis-reports --service-type RDS --identifier db-ABCDEFG1234567` → expect the created report in `AnalysisReports`; repeat with `--max-results 1` across several created reports and follow `NextToken` to page through all of them exactly once.
5. `aws pi get-resource-metrics --service-type RDS --identifier db-ABCDEFG1234567 --metric-queries '[{"Metric":"db.load.avg"}]' --start-time <t0> --end-time <t1> --period-in-seconds 60` → expect `AlignedStartTime`/`AlignedEndTime` aligned to 60s and one MetricList entry for `db.load.avg` with one DataPoint per 60s interval of the aligned window; repeating the call must return the same values.
6. `aws pi delete-performance-analysis-report --service-type RDS --identifier db-ABCDEFG1234567 --analysis-report-id <id>` → expect 200, then Get on the same id returns InvalidArgumentException (400) and List no longer includes it.
