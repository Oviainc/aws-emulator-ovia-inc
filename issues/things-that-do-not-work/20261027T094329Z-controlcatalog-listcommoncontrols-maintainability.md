Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9dfd5bd5800ceb89967be182e813f239a03c5430

# Factor the duplicated parent-summary projection shared by ListCommonControls and ListObjectives into one helper

_Filed: 2026-10-27T09:43:29Z_
_Operation: ListCommonControls | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9dfd5bd5800ceb89967be182e813f239a03c5430`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "resolve a catalog parent out of the store and project it into its `Associated*Summary` — the ARN exactly as the child stores it, and the parent's `Name` only when the parent resolves" is implemented twice in this crate's own handlers, once for `CommonControlSummary.Domain` and once for `ObjectiveSummary.Domain`.

Both copies do the same three things: look the parent ARN up in the state map (`state.domains.get(...)` / `domains.get(...)`), set `arn: Some(<child>.domain_arn.clone())` from the child's stored ARN rather than from the resolved parent, and set `name` through `Option::map` so an unresolvable parent yields `None` instead of a missing-required failure. Nothing shares the rule: `AssociatedDomainSummary` is constructed in exactly those two places in `src/` (grep for `AssociatedDomainSummary` in `src/` returns `list_common_controls.rs:97` and `list_objectives.rs:95` plus the generated type declarations), and `ListCommonControls` repeats the same shape a third time for `AssociatedObjectiveSummary` beside it.

Because the copies are independent, they can drift: if one stopped reading the parent out of the store, or started reporting the resolved parent's `arn` instead of the child's stored `domain_arn`, the two listings would describe the same domain two different ways while both still answering 200 — and a client walking domain -> objective -> common control (which `tests/all/ontology_listing.rs::the_walk_from_every_domain_reaches_the_whole_ontology` exercises) would join on inconsistent parents. The crate already made exactly this consolidation for the input side of these same two operations (`crate::state::single_resource_filter_arn`, whose doc says the rule is "this one function rather than one copy per operation", `src/state.rs:392-415`); the output-side projection is the copy that was left behind.

The fix is a single projection helper (e.g. in `crate::state` beside `single_resource_filter_arn`, or in a shared ops module) that both summaries call. Every response byte and every existing test must stay unchanged — this is a no-behavior-change consolidation.

## Evidence

Both copies are in this crate, in two sibling handlers.

Copy 1 — services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/list_common_controls.rs:87-100:
```
fn common_control_summary(
    common_control: &crate::state::CommonControl,
    state: &crate::state::ComAmazonawsControlcatalogControlcatalogState,
) -> CommonControlSummary {
    let domain = state.domains.get(&common_control.domain_arn);
    let objective = state.objectives.get(&common_control.objective_arn);
    CommonControlSummary {
        ...
        domain: AssociatedDomainSummary {
            arn: Some(common_control.domain_arn.clone()),
            name: domain.map(|domain| domain.name.clone()),
        },
```

Copy 2 — services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/list_objectives.rs:86-98:
```
fn objective_summary(
    objective: &crate::state::Objective,
    domains: &std::collections::BTreeMap<String, crate::state::Domain>,
) -> ObjectiveSummary {
    let domain = domains.get(&objective.domain_arn);
    ObjectiveSummary {
        ...
        domain: AssociatedDomainSummary {
            arn: Some(objective.domain_arn.clone()),
            name: domain.map(|domain| domain.name.clone()),
        },
```

The two handler docs state the one shared rule in two places — list_objectives.rs:84-85 "Project a stored objective into the modeled `ObjectiveSummary`, resolving its domain out of the same catalog ListDomains reports." and list_common_controls.rs:84-86 "Project a stored common control into the modeled `CommonControlSummary`, resolving its domain and objective out of the same catalog ListDomains and ListObjectives report."

The precedent for consolidating a rule shared by these same two operations is this crate's own helper — src/state.rs:395-398: "Every list operation that takes a resource filter takes the same kind of filter, so the rule is this one function rather than one copy per operation".

Existing tests that pin both copies' output (so a factoring that changed anything would fail):
- tests/all/ontology_listing.rs:173-184 asserts `Domain/Arn` and `Domain/Name` for every objective ListObjectives reports: "case objective {arn}: expected Domain.Arn {:?}, got {entry}".
- tests/all/single_resource_filter.rs:193-201 asserts every returned item reports the filtered-by parent, run against BOTH operations: `item[filter.parent_member]["Arn"]` with `parent_member` = "Domain" for /objectives and "Objective" for /common-controls.
- tests/all/catalog_resource_identity.rs:72-156 drives both listings' parent filters across every modeled partition.

## Steps to Reproduce

1. grep -n 'AssociatedDomainSummary' services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/*.rs — two construction sites: list_common_controls.rs:97 and list_objectives.rs:95.
2. Read services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/list_objectives.rs:86-98 and services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/list_common_controls.rs:87-100 side by side: the parent lookup, the `arn: Some(<child>.domain_arn.clone())` choice and the `name: ....map(|d| d.name.clone())` fallback are byte-for-byte the same rule written twice.
3. Note that src/state.rs:392-431 already hosts the analogous shared rule (`single_resource_filter_arn`) for the input side of these same two operations, so the crate has a home for the shared projection and this copy was simply left per-operation.
4. Factor one projection helper both summaries call and re-run `cargo test -p service-com-amazonaws-controlcatalog-controlcatalog`: tests/all/ontology_listing.rs, tests/all/single_resource_filter.rs and tests/all/catalog_resource_identity.rs must all still pass unchanged.
