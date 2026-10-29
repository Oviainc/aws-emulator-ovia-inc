Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 5b375a973f59b1ba0f091f974f00cc96321aba54

# ListRegions hand-rolls its bad-NextToken refusal instead of the crate's single invalid_next_token builder, so one bad token is worded two ways

_Filed: 2026-10-28T20:52:03Z_
_Operation: ListRegions | Class: error-response_

> _Note: this issue was identified against the code base at commit `5b375a973f59b1ba0f091f974f00cc96321aba54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::invalid_next_token()` is this crate's ONE builder for "a `NextToken` this service did not issue", and its own doc states it is shared by EVERY paginated operation of the service precisely so the refusal cannot differ between lists. Two of the service's three paginated operations reach it: `ListWorkspaceInstances` and `ListInstanceTypes` both answer a token that does not decode (or that resolves to no resume point) with `crate::state::invalid_next_token()`.

`ListRegions` — the third `@paginated` operation of the same service — does not. It builds its own `ValidationException` inline from `crate::state::field_validation_failed("NextToken", WHY)` with a hand-written sentence of its own, so the same failure (a `NextToken` this service never issued) comes back with one `Message` from two of the lists and a different `Message` from the third. That is exactly the drift the helper's doc says the helper exists to prevent, and it is the only paginated path in the crate that bypasses it.

The defect is the duplicated, divergent refusal wording for one failure class, not the status or the code: `ListRegions` already answers HTTP 400 `ValidationException` with `Reason`/`FieldList` naming `NextToken`, which is what the shared builder also produces. Routing the branch through `crate::state::invalid_next_token()` leaves the status, the code, the `Reason` and the `FieldList` member untouched and makes the one sentence the same as its two siblings'.

## Evidence

The crate's stated rule — services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/src/state.rs:2365-2374:
```
/// The modelled `ValidationException` (HTTP 400) for a `NextToken` this service did
/// not issue: one that does not decode, or that decodes to a resume point this
/// service's pagination cannot name.
///
/// Shared by every paginated operation here, so one bad token is refused the same way
/// whichever list produced it. `Reason` is the modelled enum value for a single bad
/// member, and `FieldList` names that member.
pub fn invalid_next_token() -> ValidationException {
    field_validation_failed("NextToken", "NextToken is not a token this service issued")
}
```

Sibling paginated operation 1 — src/ops/list_workspace_instances.rs:186-191:
```
                    None => {
                        return Err(ListWorkspaceInstancesErrorModeled::ValidationException(
                            crate::state::invalid_next_token(),
                        )
                        .into());
                    }
```

Sibling paginated operation 2 — src/ops/list_instance_types.rs:189-194:
```
                    None => {
                        return Err(ListInstanceTypesErrorModeled::ValidationException(
                            crate::state::invalid_next_token(),
                        )
                        .into());
                    }
```

The path that skips the helper — src/ops/list_regions.rs:174-182:
```
        if input.next_token.is_some() {
            const WHY: &str = "NextToken is not a token this service issued: ListRegions \
                               reports every supported Region in one response and returns \
                               no pagination token";
            return Err(ListRegionsErrorModeled::ValidationException(
                crate::state::field_validation_failed("NextToken", WHY),
            )
            .into());
        }
```
`ListRegions` is `@paginated(inputToken=NextToken, outputToken=NextToken, items=Regions)` (OPERATIONS.md:80), so it is one of the "every paginated operation here" the helper's doc names.

Test coverage: tests/all/region_catalog.rs:268-325 `a_next_token_is_refused_because_this_operation_issues_none` asserts only the HTTP status (400) and the `__type` code (`ValidationException`) across eight token shapes — it pins no message text, and a grep of tests/ for `NextToken is not a token` returns no matches, so no existing test depends on either wording.

## Steps to Reproduce

1. Open services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/src/state.rs and read the doc of `invalid_next_token` at lines 2365-2374: "Shared by every paginated operation here, so one bad token is refused the same way whichever list produced it."
2. Grep the crate for `invalid_next_token` and observe only two call sites: src/ops/list_workspace_instances.rs:188 and src/ops/list_instance_types.rs:192.
3. Open src/ops/list_regions.rs:174-182 and observe the third `@paginated` operation builds its own `ValidationException` from `field_validation_failed("NextToken", WHY)` with its own sentence instead.
4. Call `ListRegions` with `{"NextToken": "not-a-token"}` and then `ListWorkspaceInstances` with the same body: both answer HTTP 400 `ValidationException` naming `NextToken`, but with two different `Message` strings for the same failure class.
