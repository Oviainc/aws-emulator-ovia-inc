Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 4fd05cf1a47f5cbbe05dba0186301153f554fae3

# The history read decides maxResults, orderByTime, the window and the interpolation before it resolves the workspace, so an unknown workspace is answered 400 instead of the 404 this operation models

_Filed: 2026-10-29T14:24:50Z_
_Operation: GetPropertyValueHistory | Class: error-response_

> _Note: this issue was identified against the code base at commit `4fd05cf1a47f5cbbe05dba0186301153f554fae3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every workspace-scoped paginated operation of this crate resolves the SCOPE (the workspace, then the entity/component inside it) before it looks at the request's own page/shaping members, and `list_component_types.rs` states that order is deliberate and names `ListEntities` as the operation it follows. `GetPropertyValueHistory` does the reverse: `page_size`, `time_order`, `Window::of` and `Interpolation::of` all run (and all refuse with the modelled `ValidationException`, HTTP 400) before the store is reached and the workspace is looked up.

Because of that, a request naming a workspace this service does not hold together with ANY of those members being unservable is refused for the member instead of for the workspace: `GetPropertyValueHistory(workspaceId=<unknown>, maxResults=0)` answers 400 `ValidationException` "maxResults must be at least 1; omit it for the default of 25", and `orderByTime="NEWEST"`, `startTime="nope"`, a reversed window or an `interpolation` without an `intervalInSeconds` behave the same way. The handler's own `not_found` helper and the suite that drives it both say the answer for an unknown workspace is a 404 `ResourceNotFoundException` — the status a client branches on — so the ordering turns a 404 into a 400 whose message never names the workspace that could not be resolved.

Nothing in the file states a reason for deciding these members before the scope: the comment above the page-size call only restates the shared page-size rule, and the window/interpolation values are not needed until `read_series` runs inside the locked block below the lookup (the sibling `ListProperties` computes its page size AFTER its three scope lookups and uses it afterwards just the same). This is the same defect already recorded for `ExecuteQuery` (20261029T090255Z-iottwinmaker-executequery-error-response), whose evidence enumerates "every other workspace-scoped paginated handler" and omits the two data-plane reads — which are exactly the two that break the rule.

## Evidence

THE RULE A SIBLING STATES — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/list_component_types.rs:227-238:
```
        // The SCOPE is resolved before the request's own members, so a listing
        // of a workspace this service does not hold is refused for the workspace
        // whatever else the request carries — the order `ListEntities` resolves
        // them in. ...
        let Some(workspace) = state.workspaces.get_mut(&input.workspace_id) else {
            return Err(invalid(crate::state::workspace_not_found_message(
                &input.workspace_id,
            ))
            .into());
        };
```
and the test that pins it, tests/all/component_type_listing.rs:288-327:
```
/// RULE: the listing is scoped to a workspace this service holds — a request
/// naming one it does not is refused with the modelled `ValidationException`
/// naming that workspace, whatever page or filter members it carries, because
/// the scope is resolved before the request's own members.
...
        // Below the documented minimum of 1: the scope is still what the
        // refusal names.
        ("pageSizeBelowTheMinimum", json!({"maxResults": 0})),
```

THE HANDLER THAT REVERSES IT — src/ops/get_property_value_history.rs:219-230 (four refusal classes, all 400, decided FIRST):
```
        // The page size served, under the rule every paginated operation of
        // this service shares: THIS member documents a minimum of 1 and the
        // service's default of 25, so an explicit zero is refused and a request
        // that names none is served that default.
        let page_size = crate::state::page_size(
            input.max_results,
            crate::state::PageSizeRule::minimum_one(crate::state::DEFAULT_PAGE_SIZE),
        )
        .map_err(invalid)?;
        let descending = time_order(&input)?;
        let window = Window::of(&input)?;
        let interpolation = Interpolation::of(&input, &window)?;
```
and the workspace lookup, which only happens afterwards — src/ops/get_property_value_history.rs:244-251:
```
            let state = state.lock();
            let state = state.region(&region);

            let Some(workspace) = state.workspaces.get(&input.workspace_id) else {
                return Err(not_found(crate::state::workspace_not_found_message(
                    &input.workspace_id,
                )));
            };
```
with `not_found` building the 404 the operation models — src/ops/get_property_value_history.rs:335-341:
```
/// Answer that something the request named does not exist.
fn not_found(message: String) -> GetPropertyValueHistoryError {
    GetPropertyValueHistoryErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
```

TEST COVERAGE: tests/all/time_series_property_values.rs:942-954 pins the 404 for an unknown workspace, but only for a request whose other members are all servable:
```
        Refusal {
            case: "a workspace the store does not hold",
            workspace_id: "nosuchworkspace",
            request: || {
                json!({
                    "entityId": "unit-1",
                    "componentName": "Sensor",
                    "selectedProperties": ["Temp"],
                })
            },
            status: 404,
            error: "ResourceNotFoundException",
        },
```
and tests/all/time_series_property_values.rs:1018-1031 pins the 400 for `maxResults: 0` only against the KNOWN workspace (`workspace_id: WORKSPACE`). No row combines the two, so nothing pins the current order.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws") and seed an entity with a time-series property (any workspace will do; the request below names a workspace that does not exist).
2. GetPropertyValueHistory(workspaceId="nosuchworkspace", entityId="unit-1", componentName="Sensor", selectedProperties=["Temp"]) -> 404 ResourceNotFoundException naming nosuchworkspace (the answer tests/all/time_series_property_values.rs:942-954 pins).
3. Repeat the same call with maxResults=0 added -> 400 ValidationException "maxResults must be at least 1; omit it for the default of 25": the workspace is never named and the status a client branches on changes.
4. Same with orderByTime="NEWEST", or startTime="nope", or startTime later than endTime, or interpolation={"interpolationType":"LINEAR"} with no intervalInSeconds -> 400 for the member, never the 404 for the scope.
5. Compare ListComponentTypes(workspaceId="nosuchworkspace", maxResults=0) -> the refusal names the workspace (tests/all/component_type_listing.rs:309-327).
