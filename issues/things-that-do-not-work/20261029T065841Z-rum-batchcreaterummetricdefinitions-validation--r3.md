Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 5d001f011ed28660c188b78afabc22e4b6c273f2

# A metric definition carrying DimensionKeys is accepted for an Evidently destination, although the member documents itself as CloudWatch-only and the crate refuses every sibling "use only if Destination is X" pairing

_Filed: 2026-10-29T06:58:41Z_
_Operation: BatchCreateRumMetricDefinitions | Class: validation_

> _Note: this issue was identified against the code base at commit `5d001f011ed28660c188b78afabc22e4b6c273f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`MetricDefinitionRequest$DimensionKeys` opens with a destination-scoping rule: "Use this field only if you are sending the metric to CloudWatch." The dimensions a definition declares are the CloudWatch dimensions RUM associates with the metric, so a definition sent to an Evidently experiment may not carry them.

The crate never applies that rule. `crate::state::metric_definition_refusal` is the ONE place a `MetricDefinitionRequest` is judged (the batch create gates each entry on it, `UpdateRumMetricDefinition` gates its single definition on it), and its signature takes only the definition — the request's `Destination` is not passed in, so the check cannot be made there and is made nowhere else. A `BatchCreateRumMetricDefinitions` with `Destination=Evidently` and a definition carrying `DimensionKeys` answers 200, mints an id, persists the map, and `BatchGetRumMetricDefinitions` reports it back.

This is the SAME rule shape the crate already refuses twice over, from the same model paragraph style: `crate::state::metric_destination_choice_refusal` refuses a `DestinationArn` paired with `Destination=CloudWatch` on the strength of `PutRumMetricsDestinationRequest$DestinationArn`'s "Use this parameter only if `Destination` is `Evidently`", and `metric_destination_role_refusal` refuses an `IamRoleArn` the same way. Both are called by this very handler, two statements before the per-definition judge that skips the third pairing.

Distinct from the two FIXED `(…, validation)` issues about this paragraph: `20261028T164629Z-rum-batchcreaterummetricdefinitions-validation--r2` covers which ENTRY PAIRS an extended metric may carry, and `20261028T055857Z-rum-updaterummetricdefinition-validation` covers the three custom-metric pattern rules. Neither reads the request's `Destination`, which is what this rule is conditioned on.

## Evidence

Smithy, `MetricDefinitionRequest$DimensionKeys` (`smithy_query(query="doc", shape="MetricDefinitionRequest", member="DimensionKeys")`), FIRST sentence of the member doc:
"Use this field only if you are sending the metric to CloudWatch."
(followed by "This field is a map of field paths to dimension names. It defines the dimensions to associate with this metric in CloudWatch. For extended metrics, valid values for the entries in this field are the following: …")

The sibling rule of identical phrasing, which this crate DOES refuse — `services/service-com-amazonaws-rum-rum/src/state.rs:1619-1654`:
```
/// From the model: `PutRumMetricsDestinationRequest$DestinationArn` documents "Use this
/// parameter only if Destination is Evidently" … Neither half is
/// expressible as a modelled constraint, so every operation that accepts the pair refuses
/// it here with the `ValidationException` it declares.
pub fn metric_destination_choice_refusal(
    destination: &str,
    destination_arn: Option<&str>,
) -> Option<String> {
```
and `services/service-com-amazonaws-rum-rum/src/state.rs:1663-1666` (`metric_destination_role_refusal`, same treatment for `IamRoleArn`).

The judge that cannot apply the DimensionKeys rule — `services/service-com-amazonaws-rum-rum/src/state.rs:1881`:
```
pub fn metric_definition_refusal(definition: &MetricDefinitionRequest) -> Option<String> {
```
No `destination` parameter; its own doc (state.rs:1870-1875) states only the OTHER DimensionKeys rules ("For both extended metrics and custom metrics, all dimensions listed in this field must also be included in `EventPattern`" and the pinned extended entries), never the CloudWatch-only scoping.

Call sites that accept whatever that judge admits:
`services/service-com-amazonaws-rum-rum/src/ops/batch_create_rum_metric_definitions.rs:334`
```
            if let Some(message) = crate::state::metric_definition_refusal(requested) {
```
— reached for every destination, two statements after the same handler applied the destination pairing rule at `batch_create_rum_metric_definitions.rs:234-243`; and `services/service-com-amazonaws-rum-rum/src/ops/update_rum_metric_definition.rs:200`.

Test coverage: the Evidently rows of `services/service-com-amazonaws-rum-rum/tests/all/metric_definitions.rs:605-622` ("one custom metric, an Evidently experiment", "two definitions, another Evidently experiment") carry NO `DimensionKeys` — the dimension-carrying helper `extended_with_dimensions(...)` is used only on CloudWatch rows (metric_definitions.rs:591-594) — and `tests/all/metric_definition_validation.rs`'s refusal table has no row pairing `DimensionKeys` with an Evidently destination. So no existing test pins the current acceptance, and no comment in the crate states a reason for it.

## Steps to Reproduce

1. CreateAppMonitor(Name="m", Domain="example.com")
2. PutRumMetricsDestination(AppMonitorName="m", Destination="Evidently", DestinationArn=<experiment ARN>, IamRoleArn=<role ARN the caller may pass>)
3. BatchCreateRumMetricDefinitions(AppMonitorName="m", Destination="Evidently", DestinationArn=<the same experiment ARN>, MetricDefinitions=[{"Name":"JsErrorCount","EventPattern":"{\"event_type\":[\"com.amazon.rum.js_error_event\"],\"metadata\":{\"browserName\":[\"Chrome\"]}}","DimensionKeys":{"metadata.browserName":"BrowserName"}}])
4. Observed: HTTP 200 with `Errors: []` and a `MetricDefinition` carrying the `DimensionKeys` map, which `BatchGetRumMetricDefinitions(AppMonitorName="m", Destination="Evidently", DestinationArn=…)` then reports. Expected: the definition is refused — a per-definition entry in the batch create's `@required` `Errors` list (the mechanism the handler already builds for `metric_definition_refusal`), because the member is documented "Use this field only if you are sending the metric to CloudWatch".
5. Same request through UpdateRumMetricDefinition(AppMonitorName="m", Destination="Evidently", DestinationArn=…, MetricDefinitionId=<id>, MetricDefinition=<the same definition>): observed HTTP 200 and the definition replaced; expected the `ValidationException` that operation models, exactly as it answers for the `Destination`/`DestinationArn` pairing today.
6. Contrast, same handler, one statement earlier: BatchCreateRumMetricDefinitions(AppMonitorName="m", Destination="CloudWatch", DestinationArn=<experiment ARN>, MetricDefinitions=[…]) is refused with ValidationException by `metric_destination_choice_refusal` for the identically-worded "Use this parameter only if Destination is Evidently".
