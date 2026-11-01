Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 05bbbfdc708f909c61f7a8cb3ec61be4744981df

# A DETAILED finding's MetricGraph merges a request-based SLO's two query arrays into one, so the reported MetricDataQueries can carry the same Id twice

_Filed: 2026-10-29T11:22:08Z_
_Operation: ListAuditFindings | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `05bbbfdc708f909c61f7a8cb3ec61be4744981df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`assessed_metric_graph` builds the `MetricGraph` a DETAILED audit finding carries by CONCATENATING the stored query arrays of the assessed objective's indicator into ONE `MetricDataQueries` list. For a request-based SLO that is two distinct source arrays — `TotalRequestCountMetric` and the `GoodCountMetric`/`BadCountMetric` of `MonitoredRequestCountMetric` — each of which the model constrains for Id uniqueness only WITHIN ITSELF.

So a request-based SLO created with `TotalRequestCountMetric: [{"Id": "m1", ...}]` and `MonitoredRequestCountMetric: {"GoodCountMetric": [{"Id": "m1", ...}]}` — a request that satisfies the documented Id rule in both arrays, since each holds one query — is reported by `ListAuditFindings` with a single `MetricGraph.MetricDataQueries` array holding TWO queries whose `Id` is `m1`. `MetricGraph.MetricDataQueries` targets the very same `MetricDataQueries` shape, whose member `Id` is documented as having to be unique within such an array, so the emulator emits a document the model says cannot exist, and a client that keys the returned queries by `Id` (which is what the member is for: "a short name used to tie this object to the results in the response") loses one of them.

The crate already reasons about exactly this rule elsewhere: `MATERIALISED_SLI_QUERY_ID` quotes the uniqueness sentence and justifies its single fixed id by the fact that the list it goes into holds exactly one query. The merge in `assessed_metric_graph` is the one place in the crate that puts queries from two different arrays into one array and does not consider it.

This is the only place in the crate that produces a `MetricDataQueries` array out of more than one stored array, so the defect is confined to the DETAILED audit finding of a request-based objective.

## Evidence

THE MODEL RULE — `smithy_query(query="shape", shape="MetricDataQuery")`, member `Id`:

    "A short name used to tie this object to the results in the response. This `Id` must be unique within a `MetricDataQueries` array. ... The valid characters are letters, numbers, and underscore. The first character must be a lowercase letter."

and `smithy_query(query="shape", shape="MetricGraph")` shows the graph's list is that same shape: `"MetricDataQueries": {"target": "MetricDataQueries", "type": "list", ...}`, with `MetricDataQueries` = `{"type": "list", "member": "MetricDataQuery"}`.

THE CODE THAT BREAKS IT — services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:3580-3596:

    let mut queries: Vec<MetricDataQuery> = Vec::new();
    if let Some(sli) = slo.sli.as_ref() {
        queries.extend(sli.sli_metric.metric_data_queries.iter().cloned());
    }
    if let Some(request_based_sli) = slo.request_based_sli.as_ref() {
        let metric = &request_based_sli.request_based_sli_metric;
        queries.extend(metric.total_request_count_metric.iter().cloned());
        let monitored = match &metric.monitored_request_count_metric {
            MonitoredRequestCountMetricDataQueries::GoodCountMetric(queries) => queries,
            MonitoredRequestCountMetricDataQueries::BadCountMetric(queries) => queries,
        };
        queries.extend(monitored.iter().cloned());
    }
    MetricGraph {
        metric_data_queries: (!queries.is_empty()).then_some(queries),

THE STORED ARRAYS ARE THE REQUEST'S, VERBATIM — state.rs:1122-1131 (`request_based_sli_from_config`):

    total_request_count_metric: metric
        .total_request_count_metric
        .clone()
        .unwrap_or_default(),
    monitored_request_count_metric: metric
        .monitored_request_count_metric
        .clone()
        .unwrap_or(MonitoredRequestCountMetricDataQueries::GoodCountMetric(
            Vec::new(),
        )),

THE CRATE ITSELF QUOTES THE RULE IT BREAKS — state.rs:154-162:

    /// The `Id` of the one `MetricDataQuery` an SLI config's own metric description is
    /// materialised into ([`materialised_sli_metric_data_queries`]).
    ///
    /// `MetricDataQuery.Id` is `@required` and its doc states the rules an id must meet: it
    /// "must be unique within a `MetricDataQueries` array" ... A
    /// materialised list holds exactly ONE query, so a single fixed id that starts with a
    /// lowercase letter satisfies both.
    const MATERIALISED_SLI_QUERY_ID: &str = "m1";

AND THE READER THAT SURFACES IT — src/ops/list_audit_findings.rs:223 (`detailed`) into state.rs:3697-3698:

    metric_graph: detailed
        .then(|| assessed_metric_graph(slo, window_start, window_end)),

TEST COVERAGE — the collision is UNPINNED: tests/all/audit_findings.rs:890-922 is the only row that exercises a request-based objective's graph and it gives the two arrays DIFFERENT ids, then asserts the merged order:

    "TotalRequestCountMetric": [
        metric_query("total", "ApplicationSignals", "Request")
    ],
    "MonitoredRequestCountMetric": {
        "GoodCountMetric": [
            metric_query("good", "ApplicationSignals", "Success")
        ]
    }
    ...
    ("checkout-requests", vec!["total", "good"]),

so no test states what happens when both arrays use one id.

## Steps to Reproduce

1. Start the emulator with this crate mounted and point an `application-signals` client at it.
2. POST /slo with {"Name":"checkout-requests","RequestBasedSliConfig":{"RequestBasedSliMetricConfig":{"KeyAttributes":{"Type":"Service","Name":"checkout","Environment":"prod"},"TotalRequestCountMetric":[{"Id":"m1","MetricStat":{"Metric":{"Namespace":"ApplicationSignals","MetricName":"Request"},"Period":60,"Stat":"Sum"}}],"MonitoredRequestCountMetric":{"GoodCountMetric":[{"Id":"m1","MetricStat":{"Metric":{"Namespace":"ApplicationSignals","MetricName":"Success"},"Period":60,"Stat":"Sum"}}]}}}} — accepted (each array holds one query, so each satisfies the documented per-array Id uniqueness).
3. POST /auditFindings?StartTime=<now-3600>&EndTime=<now+3600> with body {"AuditTargets":[{"Type":"service","Data":{"Service":{"Type":"Service","Name":"checkout","Environment":"prod"}}}],"DetailLevel":"DETAILED"}.
4. Read the finding's `MetricGraph.MetricDataQueries`: it holds TWO entries, both with "Id": "m1", in one array.
5. Expected: the queries reported in one `MetricDataQueries` array carry distinct `Id`s, as MetricDataQuery.Id's own doc requires of that array (and as the crate's own MATERIALISED_SLI_QUERY_ID comment reasons about).
