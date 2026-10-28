Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 3c7381d6a4cea641a6c7575601888254db6ee65d

# Factor the duplicated single-ARN narrowing-and-fingerprint rule shared by ListCommonControls and ListObjectives into one helper

_Filed: 2026-10-28T13:13:30Z_
_Operation: ListCommonControls | Class: maintainability_

> _Note: this issue was identified against the code base at commit `3c7381d6a4cea641a6c7575601888254db6ee65d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "a single-ARN resource filter contributes exactly one `(dimension, values)` pair to the page fingerprint — the zero-or-one canonical parent ARN it resolved to — and narrows the collection to the children whose stored parent ARN equals that ARN" is implemented twice in this crate's own handlers: once in `ListCommonControls` (objective filter over `common_controls`) and once in `ListObjectives` (domain filter over `objectives`). Both copies are the same two expressions written out per operation: `Option<String> -> Vec<String>` for the fingerprint dimension (`filter.clone().into_iter().collect()`), and the narrowing predicate `filter.as_deref().is_none_or(|arn| child.<parent>_arn == arn)`. Only the dimension name and the field accessor differ.

This is the third copy of this operation pair's shared filter rule and it is NOT the one either fixed maintainability issue consolidated: `20261025T235816Z-controlcatalog-listobjectives-maintainability.md` factored the filter READ (entry count, missing `Arn`, partition canonicalisation) into `crate::state::single_resource_filter_arn`, and `20261027T094329Z-controlcatalog-listcommoncontrols-maintainability.md` factored the output-side parent projection into `crate::state::associated_parent_summary`. What is still per-operation is the step between them: how the resolved ARN becomes a fingerprint dimension and how it narrows the collection.

Because the copies are independent they can drift, and the crate's own test coverage is already asymmetric about them: `tests/all/catalog_resource_identity.rs::a_page_token_resumes_across_partitions_of_the_same_filter` pins the token/fingerprint behaviour for `/objectives` only, so a change to the `ListCommonControls` copy's fingerprint dimension (name, or the zero-or-one value set) would silently invalidate tokens for that operation while every assertion still passed. The crate states the consolidation rule for this very operation pair twice in `src/state.rs`, so there is already a home for the helper.

The fix is one helper in `crate::state`, beside `single_resource_filter_arn`, that both handlers call to build the single fingerprint dimension and to test a child against the filter. Every response byte and every existing test must stay unchanged — this is a no-behavior-change consolidation.

## Evidence

Both copies are in this crate, in two sibling handlers.

Copy 1 — services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/list_common_controls.rs:219-233:
```
        let fingerprint = crate::state::filter_fingerprint(&[(
            "objectives",
            objective_filter.clone().into_iter().collect(),
        )]);
        let page = crate::state::page(
            "common-controls",
            &fingerprint,
            state
                .common_controls
                .iter()
                .filter(|(_, common_control)| {
                    objective_filter
                        .as_deref()
                        .is_none_or(|arn| common_control.objective_arn == arn)
                })
```

Copy 2 — services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/list_objectives.rs:209-223:
```
        let fingerprint = crate::state::filter_fingerprint(&[(
            "domains",
            domain_filter.clone().into_iter().collect(),
        )]);
        let page = crate::state::page(
            "objectives",
            &fingerprint,
            state
                .objectives
                .iter()
                .filter(|(_, objective)| {
                    domain_filter
                        .as_deref()
                        .is_none_or(|arn| objective.domain_arn == arn)
                })
```

The crate states the one-helper-per-class rule for exactly this operation pair, twice:
- services/service-com-amazonaws-controlcatalog-controlcatalog/src/state.rs:409-412 (`single_resource_filter_arn`): "Every list operation that takes a resource filter takes the same kind of filter, so the rule is this one function rather than one copy per operation".
- services/service-com-amazonaws-controlcatalog-controlcatalog/src/state.rs:508-510 (`associated_parent_summary`): "Every listing that carries a parent summary carries the same kind of summary, so the rule is this one function rather than one copy per operation and member".

And each handler's own comment says the input rule is shared with its sibling, while the narrowing beneath it is not:
- list_common_controls.rs:202-205: "are all the one rule in `crate::state::single_resource_filter_arn` — shared with the filter ListObjectives takes, because the rule, and the sentence it refuses with, belong to the class and not to the member at fault."
- list_objectives.rs:193-195: "are all the one rule in `crate::state::single_resource_filter_arn` — shared with the filter ListCommonControls takes, because it is one rule for the class and not one per operation."

Existing tests that pin both copies' behaviour (so a factoring that changed anything would fail):
- tests/all/single_resource_filter.rs:133 `an_accepted_single_entry_filter_narrows_both_lists_the_same_way` — drives both `/objectives` and `/common-controls` through the same table (`parent_member: "Domain"` / `"Objective"`, lines 113 and 122) and asserts `item[filter.parent_member]["Arn"]` on every returned item (line 195).
- tests/all/single_resource_filter.rs:242 `an_unacceptable_single_entry_filter_is_refused_identically_on_both_operations`.
- tests/all/catalog_resource_identity.rs:342-378 `a_page_token_resumes_across_partitions_of_the_same_filter` — pins the fingerprint/token behaviour for `/objectives` (`emulator.ok("/objectives?maxResults=1", &objective_filter(arn))`, line 356).
- tests/all/ontology_listing.rs — the domain -> objective -> common-control walk over the whole seeded ontology.

## Steps to Reproduce

1. Open services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/list_common_controls.rs:219-233 and services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/list_objectives.rs:209-223 side by side: the `Option<String> -> Vec<String>` fingerprint-dimension conversion and the `filter.as_deref().is_none_or(|arn| child.<parent>_arn == arn)` narrowing predicate are the same rule written twice, differing only in the dimension name ("objectives" / "domains") and the accessor (`objective_arn` / `domain_arn`).
2. grep -n 'filter_fingerprint' services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/*.rs — ListCommonControls and ListObjectives build the single-dimension form independently; ListControls and ListControlMappings each have their own multi-dimension builder, and ListDomains passes `&[]`.
3. Confirm the gap is not the one already consolidated: services/service-com-amazonaws-controlcatalog-controlcatalog/src/state.rs:429-446 holds the shared filter READ (`single_resource_filter_arn`) and src/state.rs:515-529 the shared parent projection (`associated_parent_summary`); neither covers the fingerprint dimension or the narrowing predicate.
4. Note the asymmetric coverage that lets the copies drift: tests/all/catalog_resource_identity.rs:342 exercises token resumption across partitions for `/objectives` only, with no `/common-controls` row.
5. Factor one helper in `crate::state` (beside `single_resource_filter_arn`) that returns the single fingerprint dimension and tests a child's stored parent ARN against the filter, call it from both handlers, and re-run `cargo test -p service-com-amazonaws-controlcatalog-controlcatalog`: tests/all/single_resource_filter.rs, tests/all/catalog_resource_identity.rs and tests/all/ontology_listing.rs must all pass unchanged.
