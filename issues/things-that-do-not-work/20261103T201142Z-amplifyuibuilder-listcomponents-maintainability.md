Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 8b4869ff85a1bb54e2d6ff99978d62233f666034

# ListComponents and ListThemes each hold their own copy of the same maxResults default-and-clamp computation

_Filed: 2026-11-03T20:11:42Z_
_Operation: ListComponents | Class: maintainability_

> _Note: this issue was identified against the code base at commit `8b4869ff85a1bb54e2d6ff99978d62233f666034`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The page-size derivation for an entity list — fill in the page size for a request that names no `maxResults`, assert the validator already refused anything outside the modelled range, then clamp and convert to `usize` — is written out twice, verbatim, in `list_components.rs` and `list_themes.rs` (and once more, with its own constant, in `list_codegen_jobs.rs`). The two copies are character-for-character identical, including the comment, so a change to the rule in one (say the value a missing `maxResults` takes, or dropping the clamp now that the boundary validator is in place) silently leaves the other list serving the old rule.

This is the same consolidation the crate already performed for the page WALK: `crate::state::page_of` is documented as "The one page walk of this service, shared by every paginated operation" so "the resume semantics and the refusal of an unknown token cannot drift between two of them" — but the page BOUND each of those operations hands `page_of` is still derived per handler. A single helper beside `page_of` (taking the optional `maxResults` and the operation's limit constant) would leave every response byte-identical, since both copies compute the same value from the same constant. `ListForms` is still a scaffold, so implementing it would add a third copy.

## Evidence

Copy 1 — services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/list_components.rs:156-170
        // `maxResults` targets `ListEntityLimit`, modelled `@range(min: 1, max:
        // 100)`, and the boundary validator refused anything outside it before
        // this handler ran, so the only case left is an absent value: serve the
        // largest page the model allows. ...
        let requested = input
            .max_results
            .unwrap_or(crate::state::LIST_ENTITY_PAGE_LIMIT);
        debug_assert!(
            (1..=crate::state::LIST_ENTITY_PAGE_LIMIT).contains(&requested),
            "maxResults {requested} should have been refused at the boundary"
        );
        let page_size = usize::try_from(requested.clamp(1, crate::state::LIST_ENTITY_PAGE_LIMIT))
            .expect("a page size clamped into 1..=100 fits in usize");

Copy 2 — services/.../src/ops/list_themes.rs:156-170 — the identical comment and the identical five statements, same constant:
        let requested = input
            .max_results
            .unwrap_or(crate::state::LIST_ENTITY_PAGE_LIMIT);
        debug_assert!(
            (1..=crate::state::LIST_ENTITY_PAGE_LIMIT).contains(&requested),
            "maxResults {requested} should have been refused at the boundary"
        );
        let page_size = usize::try_from(requested.clamp(1, crate::state::LIST_ENTITY_PAGE_LIMIT))
            .expect("a page size clamped into 1..=100 fits in usize");

A third, near-identical copy over its own constant — services/.../src/ops/list_codegen_jobs.rs:165-177:
        let requested = input.max_results;
        debug_assert!(
            (1..=crate::state::CODEGEN_JOB_PAGE_LIMIT).contains(&requested),
            "maxResults {requested} should have been refused at the boundary"
        );
        let page_size = usize::try_from(requested.clamp(1, crate::state::CODEGEN_JOB_PAGE_LIMIT))

The rule the crate already states for the shared half of the same computation — services/.../src/state.rs:1414-1421 (`page_of`):
    /// The one page walk of this service, shared by every paginated operation:
    ... so "An operation supplies only what is its own — the records, the page bound and the projection it reports — so the resume semantics and the refusal of an unknown token cannot drift between two of them." (src/state.rs:50-56)

Tests that pin both copies and must stay unchanged: tests/all/component_lifecycle.rs:577-700 (every `maxResults` including none at all, both published bounds, and one past each), tests/all/theme_list.rs:385-490 (the same rows for ListThemes, explicitly including "no `maxResults` at all — which takes the ..." default), tests/all/codegen_job_list.rs:216-355, tests/all/shared_page_walk.rs and tests/all/invalid_parameter_refusals.rs:285-299 (the out-of-range refusals).

## Steps to Reproduce

1. Open src/ops/list_components.rs at lines 156-170 and src/ops/list_themes.rs at lines 156-170 and compare: the comment and the five statements are identical, so the default page size and the clamp exist twice.
2. Change one copy only (e.g. make the absent-maxResults default 50 in list_components.rs): ListComponents serves 50-record pages while ListThemes still serves 100, with nothing in the crate tying the two together.
3. Contrast with the walk itself, which both operations reach through the single crate::state::page_of, where such a change cannot diverge.
