Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d9d92e1bb4d26644359d4701433c452d79a4d930

# A repeated BatchCreateRumMetricDefinitions duplicates every definition, breaking the @idempotent invariant every other write in this crate honours

_Filed: 2026-10-25T23:13:31Z_
_Operation: BatchCreateRumMetricDefinitions | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d9d92e1bb4d26644359d4701433c452d79a4d930`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model declares this operation `@idempotent`, and every other `@idempotent` write in this crate implements that declaration as "applying the same request again leaves the same stored state": `PutRumMetricsDestination` re-puts under one identity key and "adds no second entry", `UpdateRumMetricDefinition` replaces under the id it keeps "rather than the creation of a second one", `UntagResource` removes by key so "the same request applied again leaves the same tag set", and `CreateAppMonitor` refuses a repeat with `ConflictException` instead of minting a second monitor.

`BatchCreateRumMetricDefinitions` is the one exception. The handler compares the requested definitions against nothing that is already stored: for every accepted entry it mints a brand-new `MetricDefinitionId` from the state counter and inserts a record under that fresh key. So a client that retries the call the trait licences it to retry (a timed-out or re-sent batch) ends up with two stored copies of every definition: `BatchGetRumMetricDefinitions` then reports each metric twice under two ids, each copy has to be deleted separately, and each copy consumes a slot of the destination's definition capacity the same handler meters at `MAX_METRIC_DEFINITIONS_PER_DESTINATION`.

The handler has no path to the `ConflictException` (HTTP 409, "This operation attempted to create a resource that already exists") the model declares for it either: the variant is generated into the error channel and never constructed, so an already-existing definition is neither a conflict nor an idempotent no-op — it is silently duplicated.

## Evidence

Model, the trait on this operation — `smithy_query(query="operation", operation="BatchCreateRumMetricDefinitions")` reports `"idempotent": true`; `services/service-com-amazonaws-rum-rum/OPERATIONS.md:90`: "- **Traits:** `@idempotent`".

Handler side — `services/service-com-amazonaws-rum-rum/src/ops/batch_create_rum_metric_definitions.rs:339-361`, which mints a new id per requested definition with no lookup of what is already stored:
```
        let records: Vec<crate::state::MetricDefinitionRecord> = accepted
            .into_iter()
            .map(|requested| {
                let id = store.mint_metric_definition_id();
                crate::state::MetricDefinitionRecord::from_request(&id, requested)
            })
            .collect();
...
                monitor.insert_metric_definition(&input.destination, destination_arn, record);
```
(`crate::state::MetricDefinitionRecord::from_request` is the only thing the request is run through, and `insert_metric_definition` keys on "the id in the record IS the map key", `services/service-com-amazonaws-rum-rum/src/state.rs:763-778` — so a second call cannot land on the first call's entry.)

Sibling `@idempotent` writes in the SAME crate that do leave the same state:
- `services/service-com-amazonaws-rum-rum/src/ops/put_rum_metrics_destination.rs:300-303`: "// Create-or-update under the one identity: re-putting the same destination / replaces the role it is written through and adds no second entry, so the call / is idempotent as the model declares it."
- `services/service-com-amazonaws-rum-rum/src/ops/update_rum_metric_definition.rs:272-279`: "the id is NOT part of what a caller may change — it stays the id the create minted, which is what makes this an update of that metric rather than the creation of a second one."
- `services/service-com-amazonaws-rum-rum/src/ops/untag_resource.rs:378-383`: "a key the resource does not carry (and a key named twice in one request) removes nothing and is not an error — which is what makes the operation `@idempotent`, as the model declares it: the same request applied again leaves the same tag set."
- `services/service-com-amazonaws-rum-rum/src/ops/create_app_monitor.rs:286-291`: a repeat create answers `crate::state::app_monitor_already_exists(&input.name)` rather than minting a second monitor.

The modelled 409 is declared and never used: `services/service-com-amazonaws-rum-rum/src/ops/batch_create_rum_metric_definitions.rs:136` (`ConflictException(ConflictException),`) is the only occurrence of the shape in the file besides the doc header at line 110 — `grep ConflictException` over the handler returns those two lines only.

Test coverage: the crate's suite asserts the repeat-leaves-the-same-state invariant for the siblings — `services/service-com-amazonaws-rum-rum/tests/all/metric_definitions.rs:805-811` ("// `@idempotent`: applying the same update again leaves exactly the same state." … "case {case:?}: repeating the update changed the stored definitions"), `services/service-com-amazonaws-rum-rum/tests/all/metrics_destinations.rs:366-396` ("Re-putting the SAME identity updates that destination" … "One identity, one entry"), `services/service-com-amazonaws-rum-rum/tests/all/resource_tags.rs:419-421` (`untag_resource_is_idempotent`). No test anywhere in the crate repeats a `BatchCreateRumMetricDefinitions` call, so nothing pins the duplicating behaviour with a stated reason.

## Steps to Reproduce

1. CreateAppMonitor(Name="m", Domain="example.com")
2. PutRumMetricsDestination(AppMonitorName="m", Destination="CloudWatch")
3. BatchCreateRumMetricDefinitions(AppMonitorName="m", Destination="CloudWatch", MetricDefinitions=[{"Name":"MyMetric","Namespace":"MyApp/Custom","ValueKey":"event_details.value","EventPattern":"{\"event_type\":[\"com.example.custom\"]}"}]) -> HTTP 200, one MetricDefinition with id A
4. Re-send the byte-identical request (what `@idempotent` licenses a retry to do) -> HTTP 200, a SECOND MetricDefinition with a different id B
5. BatchGetRumMetricDefinitions(AppMonitorName="m", Destination="CloudWatch"). Observed: two definitions (ids A and B) for the one metric, so the metric is defined twice and two deletes are needed to undo one create. Expected for an `@idempotent` operation, as every sibling write in this crate implements it: the second call leaves exactly the state the first one did (or is refused with the `ConflictException` the operation models).
