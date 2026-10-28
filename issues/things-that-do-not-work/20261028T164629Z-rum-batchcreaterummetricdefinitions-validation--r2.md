Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0b60124a277040298c4e92ee09b21d27ce53d94f

# An extended metric's DimensionKeys entries are never checked against the six pairs the model declares valid, so arbitrary dimension names and field paths are stored

_Filed: 2026-10-28T16:46:29Z_
_Operation: BatchCreateRumMetricDefinitions | Class: validation_

> _Note: this issue was identified against the code base at commit `0b60124a277040298c4e92ee09b21d27ce53d94f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::metric_definition_refusal` is the ONE place this crate judges a `MetricDefinitionRequest` (the batch create gates each entry on it, the update gates its single definition on it). For a definition whose `Namespace` is omitted or `AWS/RUM` it runs `extended_metric_refusal`, which checks only `Name` against the catalog and the `ValueKey`/`EventPattern` combination that row pins; then, for every kind, it checks that each `DimensionKeys` field path is named by the `EventPattern`.

The rule the model states for an extended metric's DimensionKeys ENTRIES is never applied: `MetricDefinitionRequest$DimensionKeys` enumerates the valid entries as exactly six `field path -> dimension name` pairs. Today an extended metric can be created with `{"metadata.browserName": "NotADimension"}`, or with a key that is not one of the six paths at all — including a path that is no RUM event field, since `custom_metric_refusal` (the function that applies the "all first-level keys ... must be one of the keys in the list" rule) is only reached for a CUSTOM metric. The definition is minted an id and persisted, and `BatchGetRumMetricDefinitions` reports it, where the model's answer is a refusal: a per-definition entry in the batch create's `@required` `Errors` list (the mechanism the handler already builds) and the `ValidationException` the update models.

Filed against `BatchCreateRumMetricDefinitions`, the first caller of the shared judge. Both `(BatchCreateRumMetricDefinitions, validation)` and `(UpdateRumMetricDefinition, validation)` are held by FIXED issues that covered DIFFERENT rules of the same paragraph — the omitted custom namespace and the extended Name/ValueKey/EventPattern table (`20261025T155027Z-rum-batchcreaterummetricdefinitions-validation`, both implemented today at state.rs:1491-1500 and `extended_metric_refusal`), and the three custom-metric rules (`20261028T055857Z-rum-updaterummetricdefinition-validation`, implemented today in `custom_metric_refusal`). The DimensionKeys entry set is described by neither.

## Evidence

Smithy, `MetricDefinitionRequest$DimensionKeys` (`smithy_query(query="doc", shape="MetricDefinitionRequest", member="DimensionKeys")`):
"This field is a map of field paths to dimension names. It defines the dimensions to associate with this metric in CloudWatch. For extended metrics, valid values for the entries in this field are the following:" — `"metadata.pageId": "PageId"`, `"metadata.browserName": "BrowserName"`, `"metadata.deviceType": "DeviceType"`, `"metadata.osName": "OSName"`, `"metadata.countryCode": "CountryCode"`, `"event_details.fileType": "FileType"` — "For both extended metrics and custom metrics, all dimensions listed in this field must also be included in `EventPattern`."
(The map's own modelled constraints cannot express it: `DimensionKeysMap` is `length 0..29`, key `DimensionKey` `length 1..280`, value `DimensionName` `length 1..255`, pattern `^(?!:).*[^\s].*` — so any non-blank name passes the boundary.)

Handler side — the WHOLE of what an extended metric is judged on, `services/service-com-amazonaws-rum-rum/src/state.rs:1568-1619` (`extended_metric_refusal`): the catalog row for `Name` (:1574), `declared_value_key != entry.value_key` (:1594), and `event_pattern_includes(pattern, &required)` (:1614) — `definition.dimension_keys` is never read in this function.

The only DimensionKeys check that does run, `services/service-com-amazonaws-rum-rum/src/state.rs:1550-1555`:
```
    dimensions
        .into_iter()
        .find(|field| !event_pattern_names_field(&pattern, field))
        .map(|field| {
            format!("DimensionKeys names the field {field}, which the EventPattern does not include.")
        })
```
It judges the map's KEYS against the pattern only; no code anywhere compares a key or a value against the six documented entries (`grep` for `PageId|BrowserName|DeviceType|OSName|CountryCode|FileType` in `src/` matches nothing).

And the first-level-key rule that would at least bound the key is custom-only, `services/service-com-amazonaws-rum-rum/src/state.rs:1525-1531`:
```
    if extended_namespace {
        if let Some(message) = extended_metric_refusal(definition, namespace, pattern.as_ref()) {
            return Some(message);
        }
    } else if let Some(message) = custom_metric_refusal(definition, pattern.as_ref()) {
```
with `RUM_EVENT_FIELDS` consulted only inside `custom_metric_refusal` (state.rs:1671-1682).

Call sites that accept whatever the judge answers `None` for: `services/service-com-amazonaws-rum-rum/src/ops/batch_create_rum_metric_definitions.rs:319` and `services/service-com-amazonaws-rum-rum/src/ops/update_rum_metric_definition.rs:194`.

Test coverage: `services/service-com-amazonaws-rum-rum/tests/all/metric_definitions.rs:131-135` defines `EXTENDED_DIMENSION_KEYS` as "Every additional dimension an extended metric may carry, as the model lists them" and every accepted/refused row in `metric_definitions.rs` and `metric_definition_validation.rs` draws from that array alone; `refused_definitions()` (metric_definition_validation.rs:258+) has no row for a dimension entry outside the six. No test states a reason for admitting one.

## Steps to Reproduce

1. CreateAppMonitor(Name="m", Domain="example.com")
2. PutRumMetricsDestination(AppMonitorName="m", Destination="CloudWatch")
3. BatchCreateRumMetricDefinitions(AppMonitorName="m", Destination="CloudWatch", MetricDefinitions=[{"Name":"JsErrorCount","EventPattern":"{\"event_type\":[\"com.amazon.rum.js_error_event\"],\"metadata\":{\"browserName\":[\"Chrome\"]}}","DimensionKeys":{"metadata.browserName":"NotADimension"}}]). Observed: HTTP 200, Errors=[] and a MetricDefinition with a minted id. Expected: a per-definition entry in Errors, because for extended metrics the only valid entry keyed on `metadata.browserName` is `"metadata.browserName": "BrowserName"`.
4. BatchCreateRumMetricDefinitions(... MetricDefinitions=[{"Name":"JsErrorCount","EventPattern":"{\"event_type\":[\"com.amazon.rum.js_error_event\"],\"not_a_rum_field\":{\"x\":[\"1\"]}}","DimensionKeys":{"not_a_rum_field.x":"Whatever"}}]). Observed: created and stored; expected a refusal — the entry is not one of the six the model lists for an extended metric.
5. UpdateRumMetricDefinition(AppMonitorName="m", Destination="CloudWatch", MetricDefinitionId=<id from step 3>, MetricDefinition=<the same invalid definition>). Observed: HTTP 200 and the stored definition replaced; expected ValidationException.
6. BatchGetRumMetricDefinitions(AppMonitorName="m", Destination="CloudWatch") reports the invalid dimension entries.
