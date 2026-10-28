Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 03bb735b5164c6b554d75cb5873a72ff1e3f303a

# The "is this resource set in scope" predicate is implemented twice in state.rs

_Filed: 2026-10-27T16:53:14Z_
_Operation: GetArchitectureRecommendations | Class: maintainability_

> _Note: this issue was identified against the code base at commit `03bb735b5164c6b554d75cb5873a72ff1e3f303a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule that decides whether a stored resource set holds a resource contained by a given set of readiness scopes — iterate the set's `resources`, flatten each entry's optional `readiness_scopes`, and ask whether any of them is in the scope set — is written out twice in `src/state.rs`, once inside `readiness_check_names_in_scopes` (the summary/coverage path, reached from `GetCellReadinessSummary`, `GetRecoveryGroupReadinessSummary` and the COVERAGE criterion of `assess_architecture`) and once inside `resource_types_in_scopes` (the SYMMETRY criterion of `assess_architecture`, reached from `GetArchitectureRecommendations`).

The second copy's own doc comment states that it is the same rule as the first ("the same resource-to-scope match ... read for the type rather than for the watching check"), and the module header states the rule the file is meant to follow: the shared helpers live here "so the operation files agree on one implementation of each instead of carrying a copy apiece that can drift apart". Because the two copies are separate expressions, a change to what "in scope" means — e.g. treating an entry with no `readinessScopes` as globally scoped, or matching a scope prefix instead of an exact ARN — lands in one criterion and silently leaves the other reporting the old rule: `GetCellReadinessSummary` would name a check that `GetArchitectureRecommendations` does not count toward symmetry, from the same store.

The fix is to lift the predicate into one private helper (e.g. `fn set_in_scopes(set: &ResourceSetRecord, scopes: &BTreeSet<String>) -> bool`) that both functions call. It changes no response byte and no existing test: both callers keep returning the same `BTreeSet`s.

Note this is NOT the already-fixed summary duplication (`20261025T050225Z-route53-recovery-readiness-getcellreadinesssummary-maintainability.md`), which was about the per-check summary construction in the two summary HANDLERS and was resolved by extracting `readiness_summary_in_scopes`; the membership predicate below is a different pair of copies, inside the state module, and one of them is reached only through `assess_architecture`.

## Evidence

Copy 1 — services/service-com-amazonaws-route53recoveryreadiness-route53recoveryreadiness/src/state.rs:828-846:
```
    fn readiness_check_names_in_scopes(&self, scopes: &BTreeSet<String>) -> BTreeSet<String> {
        self.readiness_checks
            .values()
            .filter(|check| {
                self.resource_sets
                    .get(&check.resource_set_name)
                    .is_some_and(|set| {
                        set.resources.iter().any(|resource| {
                            resource
                                .readiness_scopes
                                .iter()
                                .flatten()
                                .any(|scope| scopes.contains(scope))
                        })
                    })
            })
```

Copy 2 — src/state.rs:854-867:
```
    fn resource_types_in_scopes(&self, scopes: &BTreeSet<String>) -> BTreeSet<String> {
        self.resource_sets
            .values()
            .filter(|set| {
                set.resources.iter().any(|resource| {
                    resource
                        .readiness_scopes
                        .iter()
                        .flatten()
                        .any(|scope| scopes.contains(scope))
                })
            })
```

The crate admits the duplication in the second copy's doc, src/state.rs:848-853: "/// The types of the resource sets holding a resource scoped to one of `scopes`. ... /// this is what a container holds, by type: the same resource-to-scope match [`Self::readiness_check_names_in_scopes`] makes, read for the type rather than for the watching check."

And the rule the module says it exists to keep, src/state.rs:5-9: "Besides the state struct this module holds the few helpers the resource-set, readiness-check, cell and recovery-group handlers share ... so the operation files agree on one implementation of each instead of carrying a copy apiece that can drift apart."

Both copies are live code: `src/ops/get_architecture_recommendations.rs:194` calls `state.assess_architecture(&input.recovery_group_name)`, which reads BOTH — src/state.rs:905-906: `self.resource_types_in_scopes(&scopes), self.readiness_check_names_in_scopes(&scopes),`.

Tests that pin the responses a de-duplication must leave unchanged: tests/all/architecture_recommendations.rs:369 `a_report_names_one_recommendation_per_criterion_the_architecture_fails` (exercises the symmetry criterion, i.e. copy 2, and the coverage criterion, i.e. copy 1) and :630 `the_assessment_instant_holds_until_the_architecture_changes`; tests/all/readiness_summary_shared_rule.rs (the whole file drives both summaries over copy 1); tests/all/account_gate_and_not_found.rs:991 `a_cell_readiness_summary_reports_the_checks_scoped_to_that_cell`.

## Steps to Reproduce

1. Open services/service-com-amazonaws-route53recoveryreadiness-route53recoveryreadiness/src/state.rs and read lines 828-846 beside lines 854-867: the `set.resources.iter().any(|resource| resource.readiness_scopes.iter().flatten().any(|scope| scopes.contains(scope)))` predicate appears in both, once nested under the readiness-check filter and once under the resource-set filter
2. Read src/state.rs:848-853, where the second copy's doc says it is "the same resource-to-scope match [`Self::readiness_check_names_in_scopes`] makes", and src/state.rs:5-9, which states the module exists so handlers "agree on one implementation of each instead of carrying a copy apiece that can drift apart"
3. Change the predicate in ONE copy (for example make an entry whose `readinessScopes` is absent count as in every scope) and run `cargo test -p service-com-amazonaws-route53recoveryreadiness-route53recoveryreadiness`: the summary tests and the architecture-recommendation symmetry test now disagree about which resource sets a container holds, because only one criterion was updated
