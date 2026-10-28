Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 780a41c8ff20fd069d5e365e10fd3b0dbb5b515c

# An empty or whitespace-only EventPattern is normalised for validation but stored verbatim, so two spellings the state module itself calls "the same input" become two definitions for one metric

_Filed: 2026-10-28T22:00:30Z_
_Operation: BatchCreateRumMetricDefinitions | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `780a41c8ff20fd069d5e365e10fd3b0dbb5b515c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::metric_definition_refusal` normalises `EventPattern` before judging it: the value is trimmed and an empty result is read as "no pattern", under an explicit comment stating that an empty pattern IS an absent one. But `MetricDefinitionRecord::from_request` — which the file documents as "The ONE conversion from the request shape, shared by the create and the update", and which `defines`/`same_definition` compare definitions in — copies `event_pattern` verbatim, without that normalisation.

So the identity a `BatchCreateRumMetricDefinitions` is keyed by disagrees with the validation that admitted the request. A custom metric sent once with `"EventPattern": ""` (or `"   "`) and then re-sent with `EventPattern` omitted is judged the SAME definition by `metric_definition_refusal` but a DIFFERENT one by `defines`, so the second call mints a second id and the destination ends up holding two definitions for one metric — each spelling of "no pattern" buying another one, and each consuming another slot of the destination's capacity and needing its own delete. The handler's own comment claims the opposite: "a definition the destination already holds is the one a previous call created ... Nothing is stored for it, so a retried or re-sent batch leaves exactly the state its first delivery did — it also takes no second slot of the destination's capacity and needs no second delete to undo."

The crate already has this normalisation rule for the sibling optional member and states it in exactly these terms: `declared_arn` reads an absent, empty or whitespace-only `DestinationArn`/`IamRoleArn` as "no ARN" precisely so one destination "must not become a second destination". `EventPattern` is the one member where the rule is applied to the refusal but not to the stored canonical form.

This is the residual of the FIXED issue `20261025T231331Z-rum-batchcreaterummetricdefinitions-wrong-behavior` ("A repeated BatchCreateRumMetricDefinitions duplicates every definition"): that fix keyed the create on the definition, but through a canonical form that still distinguishes inputs the validator treats as equal. Nothing here asks to reverse that fix — it asks `from_request` to normalise the member the judge above it already normalises.

## Evidence

The normalisation the judge applies, with the rule stated outright — `services/service-com-amazonaws-rum-rum/src/state.rs:1559-1565`:
```
    // An empty `EventPattern` is the same input as an absent one: the member is modelled
    // `@length(min: 0, max: 4000)`, so the empty string reaches here and names no event.
    let declared = definition
        .event_pattern
        .as_deref()
        .map(str::trim)
        .filter(|pattern| !pattern.is_empty());
```

The canonical stored form that skips it — `services/service-com-amazonaws-rum-rum/src/state.rs:538-555` (`from_request`, documented "The ONE conversion from the request shape, shared by the create and the update"):
```
            event_pattern: request.event_pattern.clone(),
```
and the identity comparison built on it — `services/service-com-amazonaws-rum-rum/src/state.rs:571-575` and `:580-590`:
```
    pub fn defines(&self, request: &MetricDefinitionRequest) -> bool {
        self.name == request.name
            && self.namespace == request.namespace
            && *self == Self::from_request(&self.metric_definition_id, request)
    }
...
    pub fn same_definition(
        left: &MetricDefinitionRequest,
        right: &MetricDefinitionRequest,
    ) -> bool {
        const UNMINTED: &str = "";
        left.name == right.name
            && left.namespace == right.namespace
            && Self::from_request(UNMINTED, left) == Self::from_request(UNMINTED, right)
    }
```
(`MetricDefinitionRecord` derives `PartialEq`, state.rs:514, so `event_pattern: Some("")` != `event_pattern: None`.)

The same rule, applied, for the sibling optional member — `services/service-com-amazonaws-rum-rum/src/state.rs:424-443`:
```
/// The ARN a request declares, with an absent, empty or whitespace-only value all read
/// as "no ARN".
...
/// whitespace is gone, and a value with nothing left names no experiment and no role: it
/// is the same input as an absent one, and must not become a second destination.
pub fn declared_arn(arn: Option<&str>) -> Option<&str> {
    arn.map(str::trim).filter(|arn| !arn.is_empty())
}
```

The claim the handler makes about what the identity guarantees — `services/service-com-amazonaws-rum-rum/src/ops/batch_create_rum_metric_definitions.rs:324-338`:
```
            // The operation is `@idempotent`, so the identity this create is keyed by is
            // the definition itself: a definition the destination already holds is the one
            // a previous call created, and applying the same request again names THAT
            // definition, under the id that call minted. Nothing is stored for it, so a
            // retried or re-sent batch leaves exactly the state its first delivery did —
            // it also takes no second slot of the destination's capacity and needs no
            // second delete to undo.
            if let Some(stored) = monitor.metric_definition_defining(
```

Test coverage: `services/service-com-amazonaws-rum-rum/tests/all/metric_definitions.rs:1572` (`a_repeated_batch_create_stores_no_second_copy_of_a_definition`) re-sends the byte-identical batch for four rows, and the definition builders it draws from (`custom`, `extended`, `with`, `metric_definitions.rs:278-320`) never produce an empty or whitespace `EventPattern`, so no test covers the two equivalent spellings. `tests/all/metric_definition_validation.rs` likewise has no row for a blank `EventPattern`, so nothing pins the current behaviour with a stated reason.

## Steps to Reproduce

1. CreateAppMonitor(Name="m", Domain="example.com")
2. PutRumMetricsDestination(AppMonitorName="m", Destination="CloudWatch")
3. BatchCreateRumMetricDefinitions(AppMonitorName="m", Destination="CloudWatch", MetricDefinitions=[{"Name":"Signups","Namespace":"MyApp/Funnel","EventPattern":""}]) -> HTTP 200, one MetricDefinition with id A (accepted, because the judge reads "" as no pattern).
4. BatchCreateRumMetricDefinitions(AppMonitorName="m", Destination="CloudWatch", MetricDefinitions=[{"Name":"Signups","Namespace":"MyApp/Funnel"}]) -> Observed: HTTP 200 with a NEW id B. Expected: id A again, because the first request's EventPattern was "the same input as an absent one".
5. Repeat with "EventPattern":"   " and "EventPattern":" " -> each spelling yields yet another id.
6. BatchGetRumMetricDefinitions(AppMonitorName="m", Destination="CloudWatch") -> Observed: four definitions all named "Signups" in namespace "MyApp/Funnel", one of which reports EventPattern "" and one "   ". Expected: one.
7. For contrast, the same shape of input on the sibling member is normalised: PutRumMetricsDestination(AppMonitorName="m", Destination="CloudWatch", DestinationArn="   ") addresses the SAME destination as DestinationArn omitted, because `declared_arn` trims it away.
