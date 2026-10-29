Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3ca5cccf5768308b81885b90d5cd9b0626890960

# UpdateRumMetricDefinition writes a definition into a destination without the definition-identity check the batch create applies, so one destination can end up holding two definitions of the same metric

_Filed: 2026-10-28T17:25:13Z_
_Operation: UpdateRumMetricDefinition | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3ca5cccf5768308b81885b90d5cd9b0626890960`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate keys a metric definition by the DEFINITION ITSELF: `BatchCreateRumMetricDefinitions` resolves a requested definition against the ones the destination already holds (`AppMonitorRecord::metric_definition_defining`) and against the earlier entries of the same batch (`MetricDefinitionRecord::same_definition`), so a destination can never come to hold two definitions that define the same metric under two different `MetricDefinitionId`s — that invariant is what makes the create `@idempotent`, and it is pinned by tests.

`UpdateRumMetricDefinition` writes into the very same `metric_definitions` map and skips both helpers. It resolves the one record the `(Destination, DestinationArn, MetricDefinitionId)` triple names and replaces it wholesale with `MetricDefinitionRecord::from_request(...)`, never asking whether some OTHER definition of that destination already defines exactly that metric. So an update of definition `id2` to the definition already stored under `id1` is accepted, and the destination is left holding two records that are identical in every member a definition consists of (Name, ValueKey, UnitLabel, DimensionKeys, EventPattern, Namespace) and differ only in the service-minted id.

That is the state the create path exists to prevent, and it breaks the create path afterwards: a later `BatchCreateRumMetricDefinitions` of that same definition resolves through `metric_definition_defining`, which answers whichever of the two duplicates sorts first by id, so one of the two stored definitions becomes unreachable through the idempotent create and the id a caller is handed back depends on which duplicate the map happens to order first. `state.rs` itself only hedges around the situation ("a destination that somehow holds two equal definitions resolves to the same one every time") rather than preventing it — and this handler is the "somehow". The operation also models `ConflictException` (HTTP 409), documented "This operation attempted to create a resource that already exists", which is the error class for exactly this condition and which this handler never constructs.

## Evidence

The identity rule the sibling create applies, `services/service-com-amazonaws-rum-rum/src/ops/batch_create_rum_metric_definitions.rs:327-350`:
```
            // The operation is `@idempotent`, so the identity this create is keyed by is
            // the definition itself: a definition the destination already holds is the one
            // a previous call created, and applying the same request again names THAT
            // definition, under the id that call minted. ...
            if let Some(stored) = monitor.metric_definition_defining(
                &input.destination,
                destination_arn,
                requested,
            ) {
                planned.push(Planned::Stored(stored.to_metric_definition()));
                continue;
            }
            // The same identity, within the one request: a definition named twice in one
            // batch is one definition too ...
            if let Some(index) = to_create.iter().position(|earlier| {
                crate::state::MetricDefinitionRecord::same_definition(earlier, requested)
            }) {
```
The helper's own statement of the rule, `services/service-com-amazonaws-rum-rum/src/state.rs:851-858`:
```
    /// What makes the create of a metric definition idempotent, as the model declares it:
    /// the identity of a definition is the definition itself ([`MetricDefinitionRecord::defines`]),
    /// so a request for a definition the destination already holds names that stored
    /// definition — under the id it already carries — rather than a second one. ...
    /// The map is ordered by id, so a destination that somehow holds two equal definitions
    /// resolves to the same one every time.
```
The update path, which calls neither helper — `services/service-com-amazonaws-rum-rum/src/ops/update_rum_metric_definition.rs:256-279`:
```
        let Some(stored) = monitor.metric_definition_mut(
            &input.destination,
            destination_arn,
            &input.metric_definition_id,
        ) else {
            ... metric_definition_not_found ...
        };

        // "Modifies one existing metric definition": the whole definition is replaced by
        // the one the request carried, and the id is NOT part of what a caller may change
        *stored = crate::state::MetricDefinitionRecord::from_request(
            &input.metric_definition_id,
            &input.metric_definition,
        );
```
The modelled error this handler never uses, OPERATIONS.md:179-181 (`UpdateRumMetricDefinition` ... `ConflictException (HTTP 409)`), whose documentation is "This operation attempted to create a resource that already exists." (`smithy_query(query="operation", operation="UpdateRumMetricDefinition")`).

Test coverage of the gap: none. `tests/all/metric_definitions.rs:1640-1712` pins the create-side rule ("repeating the batch changed the stored definitions" / "A definition that differs from a stored one in ONE member defines another metric, so it IS created — the identity is the definition, not the name it carries"), and `tests/all/metric_definition_validation.rs:429` only checks that the update judges a definition's VALIDITY the same way the create does; no test updates one definition into a copy of another, so nothing pins the duplicate as intended.

## Steps to Reproduce

1. CreateAppMonitor(Name="m", Domain="example.com")
2. PutRumMetricsDestination(AppMonitorName="m", Destination="CloudWatch")
3. BatchCreateRumMetricDefinitions(AppMonitorName="m", Destination="CloudWatch", MetricDefinitions=[A = {Name:"PageViewCount", EventPattern:'{"event_type":["com.amazon.rum.page_view_event"]}'}, B = {Name:"JsErrorCount", EventPattern:'{"event_type":["com.amazon.rum.js_error_event"]}'}]) -> two ids, id_A and id_B
4. UpdateRumMetricDefinition(AppMonitorName="m", Destination="CloudWatch", MetricDefinitionId=id_B, MetricDefinition=A). Observed: HTTP 200 and the destination now holds id_A and id_B with byte-identical definitions — the state BatchCreateRumMetricDefinitions is written never to produce.
5. BatchGetRumMetricDefinitions(AppMonitorName="m", Destination="CloudWatch") -> two MetricDefinitions that differ only in MetricDefinitionId.
6. BatchCreateRumMetricDefinitions(AppMonitorName="m", Destination="CloudWatch", MetricDefinitions=[A]) -> reports only whichever duplicate sorts first by id; the other stored copy of the same metric is now unreachable through the idempotent create.
