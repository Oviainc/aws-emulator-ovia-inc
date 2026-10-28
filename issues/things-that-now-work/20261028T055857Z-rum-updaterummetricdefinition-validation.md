Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6b193e67e38a36506ad31c413899507363d5dc88

# Three documented custom-metric validation rules are never applied, so a definition whose EventPattern/ValueKey the model declares invalid is stored

_Filed: 2026-10-28T05:58:57Z_
_Operation: UpdateRumMetricDefinition | Class: validation_

> _Note: this issue was identified against the code base at commit `6b193e67e38a36506ad31c413899507363d5dc88`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::metric_definition_refusal` is the ONE place this crate judges a `MetricDefinitionRequest`; `UpdateRumMetricDefinition` gates its single-definition write on it (update_rum_metric_definition.rs:194) and `BatchCreateRumMetricDefinitions` gates each entry of its batch on it (batch_create_rum_metric_definitions.rs:319). Its own doc comment enumerates the prose rules it implements: the reserved `AWS/` namespace, which KIND of metric the namespace makes it (with the extended-metric catalog's Name/ValueKey/EventPattern combination), `EventPattern` having to be a JSON object, and `DimensionKeys` having to be named by the pattern.

Three further rules the model states for CUSTOM metrics, in the same documentation paragraph the helper quotes from, are never applied anywhere in the crate:

1. the first-level keys used in `ValueKey`, `EventPattern` and `DimensionKeys` must be one of the fifteen RUM event fields the model then lists (`account_id`, `application_Id`, `application_version`, `application_name`, `batch_id`, `event_details`, `event_id`, `event_interaction`, `event_timestamp`, `event_type`, `event_version`, `log_stream`, `metadata`, `sessionId`, `user_details`, `userId`);
2. an `EventPattern` that carries `event_details` must also carry `event_type`;
3. inside `EventPattern`, a JSON array must carry only one value.

So a custom metric whose `ValueKey` names a field no RUM event has, or whose pattern filters on `event_details` with no `event_type`, or whose pattern lists two values in one array, is minted an id and persisted by the batch create and installed over an existing definition by the update — where the model's answer is a refusal (a per-definition entry in the batch create's `@required` `Errors` list, which that handler already builds at batch_create_rum_metric_definitions.rs:320-324, and the `ValidationException` the update models).

Filed against `UpdateRumMetricDefinition`, the single-definition caller of the shared helper: the `(BatchCreateRumMetricDefinitions, validation)` pair is already held by the FIXED issue `20261025T155027Z-rum-batchcreaterummetricdefinitions-validation`, which covered two DIFFERENT rules (a custom metric with the namespace omitted, and an extended metric whose Name/ValueKey/EventPattern combination is not the catalog's). Both of those are implemented today (state.rs:1486-1490 and `extended_metric_refusal`, state.rs:1527-1579); the three rules above are not, and are not described by that issue.

## Evidence

Smithy, `MetricDefinitionRequest` documentation (`smithy_query(query="doc", shape="MetricDefinitionRequest")`), the "For custom metrics, the following validation rules apply" list:
- "The values that you specify for ValueKey, EventPattern, and DimensionKeys must be fields in RUM events, so all first-level keys in these fields must be one of the keys in the list later in this section."
- "If EventPattern contains an event_details field, it must also contain an event_type. For every built-in event_type that you use, you must use a value for event_details that corresponds to that event_type." and "For every non-empty event_details, there must be a non-empty event_type."
- "In EventPattern, any JSON array must contain only one value."

Handler side — the WHOLE of what the shared judge checks, `services/service-com-amazonaws-rum-rum/src/state.rs:1450-1515`:
```
pub fn metric_definition_refusal(definition: &MetricDefinitionRequest) -> Option<String> {
    let namespace = definition.namespace.as_deref();
    if let Some(namespace) = namespace {
        if namespace.starts_with(RESERVED_NAMESPACE_PREFIX)
...
    let pattern = match declared {
        Some(text) => match ::serde_json::from_str::<::serde_json::Value>(text) {
            Ok(parsed) if parsed.is_object() => Some(parsed),
            Ok(_) => return Some("EventPattern must be a JSON object.".to_string()),
...
    dimensions
        .into_iter()
        .find(|field| !event_pattern_names_field(&pattern, field))
```
The parsed pattern is only ever asked two questions — is it an object (state.rs:1472-1476) and does it name each `DimensionKeys` field (state.rs:1509-1514) — and `event_pattern_names_field` (state.rs:1625-1634) walks a dotted path without ever looking at the array it ends in or at the set of permitted first-level keys. `ValueKey` is inspected only inside `extended_metric_refusal` (state.rs:1549-1560), i.e. never for a custom metric.

Its own doc list, state.rs:1434-1449, which stops short of the three rules above:
```
/// * `Namespace` (from the model): "You cannot use any string that starts with `AWS/` ...
/// * The KIND of a definition, and the rules that follow from it ...
/// * `EventPattern` (from the model): "The pattern that defines the metric, specified as a
///   JSON object", so a pattern that is not one defines nothing.
/// * `DimensionKeys` (from the model): "For both extended metrics and custom metrics, all
///   dimensions listed in this field must also be included in `EventPattern`"
```

Call sites that accept everything it answers `None` for: `services/service-com-amazonaws-rum-rum/src/ops/update_rum_metric_definition.rs:194` (`if let Some(message) = crate::state::metric_definition_refusal(&input.metric_definition)`) and `services/service-com-amazonaws-rum-rum/src/ops/batch_create_rum_metric_definitions.rs:319`.

Test coverage: `tests/all/metric_definition_validation.rs` enumerates the definitions the crate refuses in `refused_definitions()` (lines 258-309+) — every row is about the namespace or about the extended-metric combination, and there is no row for a non-RUM first-level key, for an `event_details` pattern without `event_type`, or for a multi-value array in a custom metric's pattern. The accepted row "a custom metric carrying every member" (lines 229-238) happens to use only permitted first-level keys (`"ValueKey": "event_details.latency"`), so no existing test states a reason for admitting the three cases above.

## Steps to Reproduce

1. CreateAppMonitor(Name="m", Domain="example.com")
2. PutRumMetricsDestination(AppMonitorName="m", Destination="CloudWatch")
3. BatchCreateRumMetricDefinitions(AppMonitorName="m", Destination="CloudWatch", MetricDefinitions=[{"Name":"CheckoutLatency","Namespace":"MyApp/Checkout","ValueKey":"not_a_rum_field.latency"}]) — observed HTTP 200 with Errors=[] and a minted MetricDefinitionId; expected a per-definition error, because "all first-level keys in these fields must be one of the keys in the list" and `not_a_rum_field` is not one of them
4. BatchCreateRumMetricDefinitions(... MetricDefinitions=[{"Name":"CartDepth","Namespace":"MyApp/Cart","EventPattern":"{\"event_details\":{\"latency\":[1]}}"}]) — observed created and stored; expected a refusal, because "If EventPattern contains an event_details field, it must also contain an event_type"
5. BatchCreateRumMetricDefinitions(... MetricDefinitions=[{"Name":"CartDepth2","Namespace":"MyApp/Cart","EventPattern":"{\"event_type\":[\"com.example.a\",\"com.example.b\"]}"}]) — observed created and stored; expected a refusal, because "In EventPattern, any JSON array must contain only one value"
6. UpdateRumMetricDefinition(AppMonitorName="m", Destination="CloudWatch", MetricDefinitionId=<the id from step 3>, MetricDefinition={"Name":"CheckoutLatency","Namespace":"MyApp/Checkout","EventPattern":"{\"event_details\":{\"latency\":[1]}}"}) — observed HTTP 200 and the stored definition replaced; expected ValidationException
7. BatchGetRumMetricDefinitions(AppMonitorName="m", Destination="CloudWatch") reports every one of the invalid definitions
