Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5ba3ab4ab3bba3a3a0f46bab311929e6db691ea0

# Multiple Filters are ANDed, although the model says an event matching one of the QueryFilter structures is returned

_Filed: 2026-10-29T01:32:31Z_
_Operation: GetAppMonitorData | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5ba3ab4ab3bba3a3a0f46bab311929e6db691ea0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetAppMonitorData` combines the request's `Filters` with a logical AND: an event is reported only when it satisfies EVERY `QueryFilter` the request carried. The model states the opposite quantifier in two places — `QueryFilter`'s own documentation says the returned events are those matching the values specified "in one of your QueryFilter structures", and `GetAppMonitorDataRequest$Filters` says the array filters the results "to those that match one or more sets of key-value pairs that you specify". Both sentences say an event that matches ONE of the filter structures is returned; the handler requires it to match all of them.

The effect is that a two-filter query over disjoint dimensions returns the intersection instead of the union, and the common case of asking for "events from Chrome OR from Germany" returns nothing at all, while a single-filter query behaves identically under either reading (which is why the defect is invisible for one filter).

The handler's own doc comment quotes the deciding model sentence verbatim and then draws the opposite conclusion from it ("so the filters are a conjunction"), so the reading the code implements is contradicted by the very text cited as its authority. This is reported as the semantics of combining filters only; the separate `Invert` handling and the count-based pagination of this operation are not part of this finding.

## Evidence

The model, `smithy_query(query="shape", shape="QueryFilter")` against `services/service-com-amazonaws-rum-rum/smithy-model/com-amazonaws-rum-rum.json`:

> "A structure that defines a key and values that you can use to filter the results. The only performance events that are returned are those that have values matching the ones that you specify in one of your `QueryFilter` structures."

and `smithy_query(query="shape", shape="GetAppMonitorDataRequest")`, member `Filters`:

> "An array of structures that you can use to filter the results to those that match one or more sets of key-value pairs that you specify."

The code, `services/service-com-amazonaws-rum-rum/src/ops/get_app_monitor_data.rs:433-441`:
```
/// Whether an event matches EVERY filter the request carries.
///
/// `QueryFilter` documents "The only performance events that are returned are those that
/// have values matching the ones that you specify in one of your `QueryFilter`
/// structures", so the filters are a conjunction while the `Values` of one filter are a
/// disjunction.
fn matches_filters(document: &::serde_json::Value, filters: &[QueryFilter]) -> bool {
    filters.iter().all(|filter| matches_filter(document, filter))
}
```
The comment quotes "in one of your `QueryFilter` structures" — an existential — and the line under it is `filters.iter().all(...)`, a universal. It is reached from the only place the filters are applied, same file `:252-259`:
```
            .filter(|stored| match parsed_event(&stored.event_json) {
                Some(document) => {
                    window.covers(event_seconds(&document)) && matches_filters(&document, &filters)
                }
```

Test coverage: `services/service-com-amazonaws-rum-rum/tests/all/app_monitor_data.rs:378-385` pins the AND reading —
```
        (
            "two filters with nothing in common",
            json!([
                {"Name": "Browser", "Values": ["Chrome"]},
                {"Name": "Country", "Values": ["DE"]},
            ]),
            vec![],
        ),
```
— under the test name `filters_select_the_events_matching_every_one_of_them` (`:341`) and the suite header "attributes satisfy every one of the request's filters" (`:8`). Neither the test nor its header states a model sentence, a recording or any other reason for the universal reading: the only reason stated anywhere in the crate is the handler doc comment above, which quotes the sentence that says the opposite. Rows with a single filter (`:349-369`) hold under either reading, so only the two-filter rows at `:370-385` and the "an inverted value beside a filter" row at `:408-414` would change.

## Steps to Reproduce

1. CreateAppMonitor(Name="m", Domain="example.com")
2. PutRumEvents into that monitor with two events: event 1 whose metadata reports browserName=Chrome and countryCode=US, and event 2 whose metadata reports browserName=Firefox and countryCode=DE
3. POST /appmonitor/m/data with {"TimeRange":{"After":0},"MaxResults":0,"Filters":[{"Name":"Browser","Values":["Chrome"]},{"Name":"Country","Values":["DE"]}]}
4. Observed: `Events` is empty — `matches_filters` requires the event to satisfy both structures, and no event does
5. Expected per the model sentence quoted above ("those that have values matching the ones that you specify in one of your QueryFilter structures" / "those that match one or more sets of key-value pairs"): both events are returned, event 1 because it matches the Browser structure and event 2 because it matches the Country structure
