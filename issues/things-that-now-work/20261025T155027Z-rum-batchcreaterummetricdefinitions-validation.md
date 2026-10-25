Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 9c887f34f86da2c6771f7b9b12ec1c8e6bf552af

# MetricDefinitionRequest's extended/custom-metric validation rules are not applied: a namespace-less custom metric and a mismatched extended Name/ValueKey/EventPattern are stored

_Filed: 2026-10-25T15:50:27Z_
_Operation: BatchCreateRumMetricDefinitions | Class: validation_

> _Note: this issue was identified against the code base at commit `9c887f34f86da2c6771f7b9b12ec1c8e6bf552af`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::metric_definition_refusal` is the single place both `BatchCreateRumMetricDefinitions` and `UpdateRumMetricDefinition` judge a `MetricDefinitionRequest`, and its own doc enumerates the prose rules it implements: the reserved-namespace prefix, `EventPattern` being a JSON object, and `DimensionKeys` being named by the pattern. Two documented rules of the SAME shape are never applied, so definitions the model declares invalid are minted an id and persisted:

1. A CUSTOM metric (a `Name` outside the extended-metric catalog the model lists) with `Namespace` ABSENT is accepted, although the model says a custom metric's namespace "can't be omitted". The handler only refuses a namespace that STARTS WITH `AWS/` and is not `AWS/RUM` (state.rs:1169-1179); an absent namespace never reaches a check at all.
2. An EXTENDED metric name is accepted with any `ValueKey` and any `EventPattern`, although the model pins the valid combination for each of the 13 catalog names (e.g. `JsErrorCount` must have a null `ValueKey` and an `EventPattern` including `{"event_type":["com.amazon.rum.js_error_event"]}`). Nothing in the crate ever looks at `Name` beyond copying it into the record (`MetricDefinitionRecord::from_request`, state.rs:348-362).

The same helper gates the single-definition `UpdateRumMetricDefinition`, so an update stores the same invalid definitions. Per the operation doc, the batch create's expected answer for such a definition is a per-definition entry in the `@required` `Errors` list (the mechanism the handler already has at batch_create_rum_metric_definitions.rs:313-320), not a success.

## Evidence

Smithy, `MetricDefinitionRequest` documentation (smithy_query query="doc" shape="MetricDefinitionRequest"):
- "For custom metrics, the following validation rules apply: The namespace can't be omitted and can't be AWS/RUM. You can use the AWS/RUM namespace only for extended metrics."
- "For extended metrics that are sent to the AWS/RUM namespace, the following validations apply: The Namespace parameter must be omitted or set to AWS/RUM. Only certain combinations of values for Name, ValueKey, and EventPattern are valid. … If Name is JsErrorCount, then ValueKey must be null and the EventPattern must include {\"event_type\":[\"com.amazon.rum.js_error_event\"]}"

Handler side — the ONLY namespace rule the crate applies, `services/service-com-amazonaws-rum-rum/src/state.rs:1168-1179`:
```
pub fn metric_definition_refusal(definition: &MetricDefinitionRequest) -> Option<String> {
    if let Some(namespace) = definition.namespace.as_deref() {
        if namespace.starts_with(RESERVED_NAMESPACE_PREFIX)
            && namespace != EXTENDED_METRICS_NAMESPACE
```
(`if let Some(namespace)` — a request that omits `Namespace` falls straight through; `definition.name` is never inspected anywhere in the function, state.rs:1168-1220.)

Call sites that accept whatever it returns `None` for: `services/service-com-amazonaws-rum-rum/src/ops/batch_create_rum_metric_definitions.rs:313` (`if let Some(message) = crate::state::metric_definition_refusal(requested)`) and `services/service-com-amazonaws-rum-rum/src/ops/update_rum_metric_definition.rs:194`.

The crate's own tests show both invalid shapes being accepted today:
- `services/service-com-amazonaws-rum-rum/tests/all/metric_definitions.rs:951-960`: "// And the update is judged the same way, against a definition created without a namespace at all." … `&[json!({"Name": "Seeded"})]` — a non-catalog name with no `Namespace`, created successfully.
- `services/service-com-amazonaws-rum-rum/tests/all/metric_definitions.rs:436-445`: `catalog_definitions()` gives EVERY one of the 11 extended names the same `event_pattern("com.amazon.rum.performance_navigation_event", &[])`, including `JsErrorCount`, and the catalog test asserts all 11 are created (line 573).
Neither test states a reason for the divergence; they simply never exercise the documented combination table.

## Steps to Reproduce

1. CreateAppMonitor(Name="m", Domain="example.com")
2. PutRumMetricsDestination(AppMonitorName="m", Destination="CloudWatch")
3. BatchCreateRumMetricDefinitions(AppMonitorName="m", Destination="CloudWatch", MetricDefinitions=[{"Name":"MyCustomMetric"}]) — no Namespace. Observed: HTTP 200, Errors=[] and a MetricDefinition with a minted id; expected a per-definition entry in Errors because "for custom metrics … the namespace can't be omitted".
4. BatchCreateRumMetricDefinitions(AppMonitorName="m", Destination="CloudWatch", MetricDefinitions=[{"Name":"JsErrorCount","ValueKey":"event_details.duration","EventPattern":"{\"event_type\":[\"com.amazon.rum.performance_navigation_event\"]}"}]). Observed: created and stored; expected an error entry — the model requires a null ValueKey and an EventPattern including com.amazon.rum.js_error_event for that name.
5. BatchGetRumMetricDefinitions(AppMonitorName="m", Destination="CloudWatch") reports both invalid definitions.
