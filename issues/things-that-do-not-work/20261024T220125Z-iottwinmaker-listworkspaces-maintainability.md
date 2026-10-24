Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 564feb857476f3247a0693cb65aa2d44e3322d3a

# The maxResults page-size rule is copy-pasted byte-for-byte into six handlers, and two more copies already diverge

_Filed: 2026-10-24T22:01:25Z_
_Operation: ListWorkspaces | Class: maintainability_

> _Note: this issue was identified against the code base at commit `564feb857476f3247a0693cb65aa2d44e3322d3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Six handlers in this crate carry the SAME page-size rule — "no maxResults => crate::state::DEFAULT_PAGE_SIZE, an explicit value below 1 => refuse, otherwise clamp to crate::state::MAX_PAGE_SIZE" — as six independent copies of the same three-arm match (branch + computation + refusal), not as one shared function. GetPropertyValue already proves one function is enough: it extracted the identical rule into a local `fn page_size(max_results: Option<i32>)`, while the five listing handlers each inline it.

The copies can (and already do) drift: ListScenes and ListSyncJobs implement the same conversion with the opposite failure mode and their comments argue the five inlined copies are wrong — they fail CLOSED on the `usize::try_from` arm precisely so that "a page size nothing could have asked for can never resolve to the most permissive answer", whereas the six copies fail OPEN with `.unwrap_or(crate::state::MAX_PAGE_SIZE)`. So the crate holds two different answers to one question in eight places, and a change to the rule (the default, the minimum, the ceiling, the conversion fallback, or the refusal text) has to be made in all of them by hand.

Fix shape: hoist the rule next to `DEFAULT_PAGE_SIZE`/`MAX_PAGE_SIZE` in `crate::state` as one helper returning `Result<usize, String>` (the message stays the exact string it is today) and let each handler wrap the message in its own modelled `ValidationException`. Every response byte and every existing test stays unchanged; ListScenes'/ListSyncJobs' own documented defaults and zero-handling remain their own, expressed through the same helper's parameters or left as they are.

## Evidence

Identical copy 1 — services/.../src/ops/list_workspaces.rs:194-205:
```
        let page_size = match input.max_results {
            None => crate::state::DEFAULT_PAGE_SIZE,
            Some(requested) if requested < 1 => {
                return Err(invalid(
                    "maxResults must be at least 1; omit it for the default of 25".to_string(),
                )
                .into())
            }
            Some(requested) => usize::try_from(requested)
                .unwrap_or(crate::state::MAX_PAGE_SIZE)
                .min(crate::state::MAX_PAGE_SIZE),
        };
```
Identical copy 2 — services/.../src/ops/list_metadata_transfer_jobs.rs:293-304 (byte-for-byte the same match, verified by reading both spans).
Identical copy 3 — services/.../src/ops/list_entities.rs:218-229.
Identical copy 4 — services/.../src/ops/list_component_types.rs:271-282.
Identical copy 5 — services/.../src/ops/list_tags_for_resource.rs:208-219.
Copy 6, already extracted into a function in its own handler — services/.../src/ops/get_property_value.rs:299-309:
```
fn page_size(max_results: Option<i32>) -> Result<usize, GetPropertyValueError> {
    match max_results {
        None => Ok(crate::state::DEFAULT_PAGE_SIZE),
        Some(requested) if requested < 1 => Err(invalid(
            "maxResults must be at least 1; omit it for the default of 25".to_string(),
        )),
        Some(requested) => Ok(usize::try_from(requested)
            .unwrap_or(crate::state::MAX_PAGE_SIZE)
            .min(crate::state::MAX_PAGE_SIZE)),
    }
}
```
The drift, stated by the crate itself — services/.../src/ops/list_sync_jobs.rs:234-246 (and the same comment at list_scenes.rs:242-254):
```
            // The conversion can only fail on a NEGATIVE value, which the
            // modelled range refuses before this handler runs — so this arm is
            // belt-and-braces. It fails CLOSED (the modelled refusal, naming
            // the member and the published range) rather than falling back to
            // the largest page, so a page size nothing could have asked for can
            // never resolve to the most permissive answer.
            Some(requested) => usize::try_from(requested)
                .map_err(|_| { invalid(format!(
                        "maxResults must be between 0 and {MODELLED_MAX_RESULTS}, got {requested}"
                    )) })?
                .min(crate::state::MAX_PAGE_SIZE),
```
Tests that pin the behaviour the fix must not change: tests/all/workspace_listing.rs:277-369 ("a request naming no maxResults is served the documented default of 25", plus the refusal case), tests/all/component_type_listing.rs:309 ("pageSizeBelowTheMinimum", json!({"maxResults": 0})) and :394-492, tests/all/workspace_entity_lifecycle.rs:633-736, tests/all/resource_tags.rs:623-628 ("case \"maxResultsZero\": expected a 400 ValidationException"), tests/all/metadata_transfer_job_listing.rs:636-641 ("maxResults of zero"), tests/all/property_value_read.rs:777-783 ("maxResults below the documented minimum of 1").

## Steps to Reproduce

1. Read services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/list_workspaces.rs:194-205 and services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/list_metadata_transfer_jobs.rs:293-304 — the two spans are byte-for-byte identical.
2. Read list_entities.rs:218-229, list_component_types.rs:271-282 and list_tags_for_resource.rs:208-219 — three more identical copies of the same match, including the same refusal string.
3. Read get_property_value.rs:299-309 — the same rule, already extracted into a `page_size` function, showing one function covers it.
4. Read list_sync_jobs.rs:232-247 and list_scenes.rs:240-255 — the same conversion written with the opposite (fail-closed) fallback, whose comment states that the fail-open fallback used by the six copies must not be taken.
5. Change the rule in one copy (for example the clamp or the refusal text) and observe that the other seven still carry the old version, with no compile-time link between them.
