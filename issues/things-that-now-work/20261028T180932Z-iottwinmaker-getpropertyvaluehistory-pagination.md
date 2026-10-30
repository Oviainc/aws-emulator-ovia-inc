Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 5cb591631d698c9098c00693287b08686d77b0d1

# A history nextToken is accepted by a query with different propertyFilters or interpolation, so its row ordinal resumes into a different value sequence

_Filed: 2026-10-28T18:09:32Z_
_Operation: GetPropertyValueHistory | Class: pagination_

> _Note: this issue was identified against the code base at commit `5cb591631d698c9098c00693287b08686d77b0d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetPropertyValueHistory` pages with an ORDINAL token: `page()` reads an integer out of the token and uses it as `skip`/`total` arithmetic over the value sequence the request produced. Because an ordinal only means something inside one sequence, the handler binds the token to the request members that decide that sequence — and its own doc comment states exactly that rule.

But `token_scope` binds only eight members: `entityId`, `componentName`, `componentPath`, `componentTypeId`, `orderByTime`, the start bound, the end bound and the sorted selection. It does NOT bind `propertyFilters` or `interpolation`, and both of those decide which values the page is of just as much as the window and the ordering that ARE bound:

* `read_series` drops every value no filter on that property selects, so adding/removing/changing a `propertyFilters` entry changes both which values are in the sequence and how many there are;
* the `interpolation` pass REPLACES each series' values with the resampled points, so adding or dropping `interpolation` (or changing `intervalInSeconds`) changes the sequence wholesale.

So a token minted by one query is accepted by a different query and its ordinal is applied to a sequence it was never counted in. The caller is handed a page with values silently skipped or repeated, or an empty `propertyValues` with no `nextToken` that looks like a completed walk, instead of the refusal the handler gives for exactly the same mistake on `orderByTime`, `startTime` or `endTime`. The crate's own token decoder states the intent it is meant to serve: "a token whose envelope, arity or scope disagrees is rejected rather than resumed".

(The sibling `GetPropertyValue` tabular token deliberately binds no `tabularConditions` — but it binds NO sequence-deciding member at all, so it is self-consistent. `GetPropertyValueHistory` binds some of them and omits the other two, which is what breaks its own stated rule. The key-based listings — `ListSyncResources`, `ListMetadataTransferJobs` — resume at a stored key, where a filter change cannot misalign anything.)

## Evidence

THE RULE THE HANDLER STATES — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/get_property_value_history.rs:1049-1051:
```
/// What a `nextToken` is bound to besides its workspace: the members that
/// decide WHICH values of which components the page is of, so a token is only
/// resumed by the query that minted it.
```

WHAT IT ACTUALLY BINDS — src/ops/get_property_value_history.rs:1057-1072 (no `propertyFilters`, no `interpolation`):
```
fn token_scope(input: &GetPropertyValueHistoryInput) -> String {
    let mut selected = input.selected_properties.clone();
    selected.sort();
    selected.dedup();
    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}",
        input.entity_id.as_deref().unwrap_or(""),
        input.component_name.as_deref().unwrap_or(""),
        input.component_path.as_deref().unwrap_or(""),
        input.component_type_id.as_deref().unwrap_or(""),
        input.order_by_time.as_deref().unwrap_or(ORDER_ASCENDING),
        bound_key(input.start_time.as_deref(), input.start_date_time.as_ref()),
        bound_key(input.end_time.as_deref(), input.end_date_time.as_ref()),
        selected.join(","),
    )
}
```

PROOF THAT THE TWO OMITTED MEMBERS DECIDE THE SEQUENCE —
* src/ops/get_property_value_history.rs:938-940 (filters remove values from the sequence):
```
                if !selects(filters, property_name, &value.value) {
                    continue;
                }
```
* src/ops/get_property_value_history.rs:282-284 (interpolation replaces the sequence):
```
                for one in &mut series {
                    one.values = interpolation.resample(&one.values);
                }
```

THE ORDINAL THE TOKEN CARRIES — src/ops/get_property_value_history.rs:997-1002 and 1036-1037:
```
        let resume_at = match input.next_token.as_deref() {
            None | Some("") => 0,
            Some(token) => decode_token(input, token)
                .ok_or_else(|| invalid(format!("nextToken {token} is not a valid token")))?,
        };
        let total: usize = series.iter().map(|one| one.values.len()).sum();
...
        let served = resume_at.saturating_add(taken);
        let next_token = (served < total).then(|| encode_token(input, served));
```

THE INTENT THE SHARED DECODER STATES — src/state.rs:4251-4253:
```
/// The key is split off first and each scope part is compared in turn, so a
/// token whose envelope, arity or scope disagrees is rejected rather than
/// resumed;
```

TEST COVERAGE: tests/all/time_series_property_values.rs:643-702 (`a_history_pages_through_every_value_once`) pins paging when the WHOLE request is repeated unchanged — the fix leaves it green. The refusal table `a_history_read_refuses_what_it_cannot_serve` (tests/all/time_series_property_values.rs:856-861, 14 cases) has no case for a token from another query at all: `grep -n 'token' tests/all/time_series_property_values.rs` returns only the paging-loop lines 667-692, so no test pins the current acceptance.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws"); create a component type with a time-series DOUBLE property "Temp" and an entity "unit-1" with a component "Sensor" instantiating it.
2. BatchPutPropertyValues on ws writing five values for unit-1/Sensor/Temp: 10, 20, 30, 40, 50 at five increasing instants.
3. POST /workspaces/ws/entity-properties/history with {"entityId":"unit-1","componentName":"Sensor","selectedProperties":["Temp"],"maxResults":2} -> 200 with the first two values and a nextToken whose ordinal is 2.
4. Repeat the request with that same nextToken but ADD {"propertyFilters":[{"propertyName":"Temp","operator":">","value":{"doubleValue":45}}]}. The filtered sequence holds one value (50), so resume_at=2 is past total=1: the answer is 200 with an empty propertyValues and no nextToken — a completed walk that reported nothing — instead of refusing a token this query did not mint.
5. Repeat step 3's token with {"interpolation":{"interpolationType":"LINEAR","intervalInSeconds":60},"startTime":<t0>,"endTime":<t4>} dropped or added relative to the minting request: the ordinal indexes into the resampled point list, so values are silently skipped or repeated.
6. Contrast a member that IS bound: repeat step 3's token with "orderByTime":"DESCENDING" (or a changed startTime) — the token is correctly refused with a ValidationException. The same class of change to propertyFilters or interpolation is not.
