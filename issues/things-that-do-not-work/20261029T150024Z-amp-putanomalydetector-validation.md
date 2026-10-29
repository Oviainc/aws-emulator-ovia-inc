Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ee0cc881641f9455cbcb1e70f91e773fdf19df64

# A Random Cut Forest query is accepted whenever it STARTS with an aggregation operator, so `sum(a) / sum(b)` is stored although no aggregation operator wraps it

_Filed: 2026-10-29T15:00:24Z_
_Operation: PutAnomalyDetector | Class: validation_

> _Note: this issue was identified against the code base at commit `ee0cc881641f9455cbcb1e70f91e773fdf19df64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::check_random_cut_forest_configuration` — the one rule `PutAnomalyDetector` (and `CreateAnomalyDetector`) applies to the stored `configuration` — decides "is this query wrapped by a supported aggregation operator?" by looking ONLY at the leading identifier of the expression (`crate::state::leading_call_operator`). It never checks that the call it found spans the WHOLE expression. So every expression that merely BEGINS with `avg|count|group|max|min|quantile|stddev|stdvar|sum` followed by `(` is accepted, whatever is applied to it afterwards:

* `sum(rate(a[5m])) / sum(rate(b[5m]))` — root is `/`
* `sum(rate(a[5m])) + 1` — root is `+`
* `sum(up) > 0` — root is `>`
* `max(up) or min(down)` — root is `or`

Each is stored verbatim as the detector's specification and reported back by `DescribeAnomalyDetector`/`ListAnomalyDetectors`, even though the aggregation is not outermost — the exact shape the crate's own helper doc and its own tests say must be refused with the 400 `ValidationException` naming `configuration.randomCutForest.query`. The mirror cases where the non-outermost aggregation is also not LEADING (`1 + sum(...)`, `(sum(...)) / count(up)`) ARE refused, so two syntactically equivalent queries get opposite answers depending only on which operand is written first.

The helper is shared, so `CreateAnomalyDetector` has the identical hole; fixing `crate::state::check_random_cut_forest_configuration` closes both.

## Evidence

MODEL (`smithy_query(query="doc", shape="RandomCutForestConfiguration", member="query")`), an `<important>` callout:
> "Random Cut Forest queries must be wrapped by a supported PromQL aggregation operator. ... Supported PromQL aggregation operators: avg, count, group, max, min, quantile, stddev, stdvar, and sum."

THE CRATE'S OWN RULE — services/service-com-amazonaws-amp-amazonprometheusservice/src/state.rs:1846-1850:
```
/// `AnomalyDetectorConfiguration` is a union whose only member is `randomCutForest`, and
/// `RandomCutForestConfiguration$query` requires the query be "wrapped by a supported
/// PromQL aggregation operator" — so the whole expression must BE a call to one of
/// [`RANDOM_CUT_FOREST_AGGREGATION_OPERATORS`].
```
WHAT THE CODE ACTUALLY DECIDES — state.rs:1857-1860 (inside `check_random_cut_forest_configuration`):
```
    let wrapping = leading_call_operator(&random_cut_forest.query);
    if wrapping.is_some_and(|operator| operators.contains(&operator)) {
        return Ok(());
    }
```
and `leading_call_operator` stops at the identifier and its opening parenthesis — state.rs:1824-1841:
```
pub fn leading_call_operator(expression: &str) -> Option<&str> {
    let trimmed = expression.trim_start();
    let end = trimmed
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(trimmed.len());
    let (identifier, rest) = trimmed.split_at(end);
    ...
    let applies = rest.starts_with('(')
        || ["by", "without"].iter().any(|modifier| { ... });
    applies.then_some(identifier)
```
Nothing after the identifier's argument list is looked at, so `sum(a) / sum(b)` returns `Some("sum")` and is accepted — while that same function's doc (state.rs:1821-1823) claims it answers `None` for "a bare selector, a parenthesised or arithmetic expression, which is exactly the shape that is not wrapped".

ThE HANDLER THAT STORES IT — src/ops/put_anomaly_detector.rs:242-244:
```
        // The configuration's own rule, shared with every operation that stores one.
        crate::state::check_random_cut_forest_configuration(&input.configuration)
            .map_err(PutAnomalyDetectorErrorModeled::ValidationException)?;
```
(and src/ops/create_anomaly_detector.rs:240-242 calls the same helper).

TEST COVERAGE that states the rule and leaves this shape untested — tests/all/anomaly_detector_update.rs:917-926:
```
    // Forms that are NOT a call to one of those operators: a bare selector, an operator
    // the catalog does not list, an aggregation that is not outermost, and a parenthesised
    // expression.
    for query in [
        "rate(requests_total[5m])",
        "requests_total",
        "topk(5, rate(requests_total[5m]))",
        "1 + sum(rate(requests_total[5m]))",
        "(sum(rate(requests_total[5m])))",
        "summary(rate(requests_total[5m]))",
    ] {
```
"an aggregation that is not outermost" is the class the suite means to refuse, but every row where the aggregation is not outermost also fails to LEAD, so no row reaches the hole. tests/all/anomaly_detector_lifecycle.rs:532-544 repeats the same list for `CreateAnomalyDetector` ("two where an aggregation appears but does not wrap the whole expression": `1 + sum(...)`, `(sum(...)) / count(up)`).

## Steps to Reproduce

1. POST /workspaces (CreateWorkspace) and wait for the workspace to report ACTIVE
2. PUT /workspaces/{workspaceId}/anomalydetectors/ad-test with {"configuration": {"randomCutForest": {"query": "sum(rate(a[5m])) / sum(rate(b[5m]))"}}, "evaluationIntervalInSeconds": 60}
3. Observe HTTP 202 and a detector created: the query's root is `/`, so no aggregation operator wraps it
4. GET /workspaces/{workspaceId}/anomalydetectors/ad-test — the unwrapped query is reported back as the stored specification
5. Repeat step 2 with {"query": "1 + sum(rate(a[5m]))"} — refused 400 ValidationException naming configuration.randomCutForest.query, although the two expressions are equally un-wrapped
