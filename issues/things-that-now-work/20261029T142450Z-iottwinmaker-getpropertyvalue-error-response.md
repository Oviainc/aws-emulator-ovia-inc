Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 4fd05cf1a47f5cbbe05dba0186301153f554fae3

# GetPropertyValue decides maxResults before it resolves the workspace, so an unknown workspace with maxResults=0 is answered 400 instead of the 404 it models

_Filed: 2026-10-29T14:24:50Z_
_Operation: GetPropertyValue | Class: error-response_

> _Note: this issue was identified against the code base at commit `4fd05cf1a47f5cbbe05dba0186301153f554fae3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetPropertyValue` computes the page size — which refuses an explicit `maxResults` of 0 with the modelled `ValidationException` (HTTP 400) — before it reaches the store and looks up the workspace. The crate states the opposite order for its scoped paginated operations in `list_component_types.rs` ("The SCOPE is resolved before the request's own members, so a listing of a workspace this service does not hold is refused for the workspace whatever else the request carries"), seven sibling handlers follow it, and a test pins it with a `maxResults: 0` row.

So `GetPropertyValue(workspaceId=<unknown>, entityId=..., componentName=..., selectedProperties=[...], maxResults=0)` answers 400 `ValidationException` "maxResults must be at least 1; omit it for the default of 25" instead of the 404 `ResourceNotFoundException` the handler's own `not_found` helper builds for an unknown workspace and the suite pins for it — a different status code, and a message that never names the workspace that could not be resolved.

No ordering reason is stated: the comment above the call restates the shared page-size rule and says the bound keeps "at most MAX_PAGE_SIZE rows ... built under the state lock", which holds just as well if the page size is computed after the lookup (the sibling `ListProperties` computes its page size after all three of its scope lookups). This is the same ordering defect already recorded for `ExecuteQuery` (20261029T090255Z-iottwinmaker-executequery-error-response), whose evidence enumerates "every other workspace-scoped paginated handler" and omits this one.

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
and tests/all/component_type_listing.rs:288-312, which pins it with the very member at issue:
```
/// RULE: the listing is scoped to a workspace this service holds — a request
/// naming one it does not is refused with the modelled `ValidationException`
/// naming that workspace, whatever page or filter members it carries, because
/// the scope is resolved before the request's own members.
...
        ("pageSizeBelowTheMinimum", json!({"maxResults": 0})),
```

THE HANDLER THAT REVERSES IT — src/ops/get_property_value.rs:206-216 (page size FIRST):
```
        // The page size served, under the rule every paginated operation of this
        // service shares (`crate::state::page_size`): THIS member documents a
        // minimum of 1 and the service's default of 25, so an explicit zero is
        // refused and a request that names none is served that default; whatever
        // is asked for, at most `crate::state::MAX_PAGE_SIZE` rows are built
        // under the state lock.
        let page_size = crate::state::page_size(
            input.max_results,
            crate::state::PageSizeRule::minimum_one(crate::state::DEFAULT_PAGE_SIZE),
        )
        .map_err(invalid)?;
```
and the workspace lookup only afterwards — src/ops/get_property_value.rs:231-238:
```
            let state = state.lock();
            let state = state.region(&region);

            let Some(workspace) = state.workspaces.get(&input.workspace_id) else {
                return Err(not_found(crate::state::workspace_not_found_message(
                    &input.workspace_id,
                )));
            };
```
with `not_found` building the 404 the operation models — src/ops/get_property_value.rs:298-304.

TEST COVERAGE: tests/all/property_value_read.rs:676-687 pins the 404 for an unknown workspace with no `maxResults` in the body:
```
        Refusal {
            what: "an unknown workspace",
            workspace_id: "no-such-workspace",
            body: json!({
                "entityId": "unit-1",
                "componentName": "Spec",
                "selectedProperties": ["Tag"],
            }),
            status: 404,
            error: "ResourceNotFoundException",
        },
```
and property_value_read.rs:776-787 pins the 400 for `maxResults: 0` only against `workspace_id: WORKSPACE`. No row combines them, so nothing pins the current order.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws") and create an entity "unit-1" with a component "Spec" carrying a property "Tag".
2. GetPropertyValue(workspaceId="no-such-workspace", entityId="unit-1", componentName="Spec", selectedProperties=["Tag"]) -> 404 ResourceNotFoundException naming no-such-workspace (tests/all/property_value_read.rs:676-687).
3. Add maxResults=0 to the same request -> 400 ValidationException "maxResults must be at least 1; omit it for the default of 25": the status changes and the workspace is never named.
4. Compare ListProperties(workspaceId="no-such-workspace", entityId="unit-1", componentName="Spec", maxResults=0) -> 404 for the workspace, because src/ops/list_properties.rs:219-229 resolves the scope before src/ops/list_properties.rs:255-259 computes the page size.
