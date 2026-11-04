Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 75da5c853dde52b5c6431a41dd7ff04ce793da30

# The recommendation-list filter predicate, timestamp bounds and cursor scope are stated twice, once per recommendation listing

_Filed: 2026-11-03T16:20:07Z_
_Operation: ListRecommendations | Class: maintainability_

> _Note: this issue was identified against the code base at commit `75da5c853dde52b5c6431a41dd7ff04ce793da30`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate deliberately states a rule ONCE when two levels (account and organization) apply it to two different stored shapes: the lifecycle write is shared through the `LifecycleWritable` trait + `apply_lifecycle_update`, and the two resource lists share `crate::state::ResourceFilters` (predicate) and `crate::state::resource_summary` (projection) — both of those were consolidated in response to earlier maintainability issues on this very crate.

The one remaining pair of sibling listings — `ListRecommendations` over `RecommendationEntry` and `ListOrganizationRecommendations` over `OrganizationRecommendationEntry` — still spells the SAME rule out twice:

1. the `afterLastUpdatedAt` / `beforeLastUpdatedAt` conversion through `crate::state::millis_of`;
2. the six equality/membership tests (`type`, `status`, `pillar`, `awsService`, `source`, `checkIdentifier`) plus the two inclusive `last_updated_at_millis` bound comparisons;
3. the eight-element cursor-scope list, in the same member names and the same order.

The two copies are structurally identical and operate on members the crate itself documents as being the same on both entry types (`state.rs:570-572`), so they can silently drift: a fix or an added filter semantics change applied to one listing leaves the other answering differently, which is exactly the failure mode `ResourceFilters` and `LifecycleWritable` exist to prevent. The only difference the account-level copy carries is the extra `is_foreign_resource` row test, which is orthogonal to the eight shared filters and can stay in the handler.

A fix is a shared predicate/scope pair in `crate::state` (as `ResourceFilters` already is) reached through a small accessor trait over the two entry types (as `LifecycleWritable` already is). Every response and every existing test stays unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/list_recommendations.rs:249-284:
```
        // `afterLastUpdatedAt` / `beforeLastUpdatedAt` are inclusive bounds on
        // the stored `lastUpdatedAt`; every other filter is an equality (or
        // membership) test. An absent filter keeps every recommendation.
        let after = input
            .after_last_updated_at
            .as_ref()
            .map(crate::state::millis_of);
...
                    && input.r#type.as_ref().is_none_or(|kind| &entry.r#type == kind)
                    && input.status.as_ref().is_none_or(|status| &entry.status == status)
                    && input
                        .pillar
                        .as_ref()
                        .is_none_or(|pillar| entry.pillars.iter().any(|stored| stored == pillar))
                    && input.aws_service.as_ref().is_none_or(|service| {
                        entry.aws_services.iter().any(|stored| stored == service)
                    })
                    && input.source.as_ref().is_none_or(|source| &entry.source == source)
                    && input
                        .check_identifier
                        .as_ref()
                        .is_none_or(|check| entry.check_arn.as_ref() == Some(check))
                    && after.is_none_or(|bound| entry.last_updated_at_millis >= bound)
                    && before.is_none_or(|bound| entry.last_updated_at_millis <= bound)
```
and its scope, list_recommendations.rs:301-315:
```
        let after_scope = after.map(|millis| millis.to_string());
        let before_scope = before.map(|millis| millis.to_string());
        let page = crate::state::paginate(
            &crate::state::list_scope(
                "recommendations",
                &[
                    ("type", input.r#type.as_deref()),
                    ("status", input.status.as_deref()),
                    ("pillar", input.pillar.as_deref()),
                    ("awsService", input.aws_service.as_deref()),
                    ("source", input.source.as_deref()),
                    ("checkIdentifier", input.check_identifier.as_deref()),
                    ("afterLastUpdatedAt", after_scope.as_deref()),
                    ("beforeLastUpdatedAt", before_scope.as_deref()),
                ],
            ),
```

Copy 2 — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/list_organization_recommendations.rs:237-275:
```
        // `afterLastUpdatedAt` / `beforeLastUpdatedAt` are inclusive bounds on
        // the stored `lastUpdatedAt`; every other filter is an equality (or
        // membership) test. An absent filter keeps every organization
        // recommendation.
        let after = input
            .after_last_updated_at
            .as_ref()
            .map(crate::state::millis_of);
...
                input.r#type.as_ref().is_none_or(|kind| &entry.r#type == kind)
                    && input
                        .status
                        .as_ref()
                        .is_none_or(|status| &entry.status == status)
                    && input
                        .pillar
                        .as_ref()
                        .is_none_or(|pillar| entry.pillars.iter().any(|stored| stored == pillar))
                    && input.aws_service.as_ref().is_none_or(|service| {
                        entry.aws_services.iter().any(|stored| stored == service)
                    })
...
                    && after.is_none_or(|bound| entry.last_updated_at_millis >= bound)
                    && before.is_none_or(|bound| entry.last_updated_at_millis <= bound)
```
and its scope, list_organization_recommendations.rs:291-305 — the same eight pairs in the same order, only the list name differing (`"organizationRecommendations"`).

The crate's own rule that such a shared rule is stated ONCE — src/state.rs:861-868 (`ResourceFilters`):
```
/// The resource filters BOTH resource lists cut their rows on, as one read's
/// request states them: `status`, `exclusionStatus` and `regionCode`, each
/// `None` when the request named it not at all.
///
/// The three are modelled identically on the two operations and select over the
/// resource's own stored members, so the predicate is stated once — by
/// [`Self::keeps`] — and the cursor scope the filters contribute once, by
/// [`Self::scope`].
```
and src/state.rs:638-643 (`LifecycleWritable`):
```
/// The two operations model the same stage and reason-code enums and carry the
/// same "only supports prioritized recommendations" sentence, so they apply ONE
/// rule to two kinds of entry. This is the seam that lets that rule be stated
/// once — by [`lifecycle_update_conflict`] and [`apply_lifecycle_update`] —
/// instead of once per operation, where the account-level and the
/// organization-level lifecycle could drift apart on the same members.
```
That the two entry types carry the same members a shared predicate would read — src/state.rs:570-572:
```
/// The organization-level view of one check's findings, so it carries the same
/// classification members as the account-level [`RecommendationEntry`] and
/// shares that check's [`TextCatalog`].
```

Test coverage that must stay green and unchanged: tests/all/recommendation_loop.rs:491 `list_recommendations_reports_the_store_and_honours_every_filter`, tests/all/organization_recommendations.rs:145 `the_organization_list_reports_the_store_and_honours_every_filter`, tests/all/cursor_scope_encoding.rs:198 `a_filter_value_that_spells_out_another_reads_scope_cannot_replay_its_token`, tests/all/recommendation_loop.rs:373 and tests/all/organization_recommendations.rs:588 (`a_next_token_this_*_did_not_issue_is_refused`), tests/all/account_scoped_recommendation_list.rs and tests/all/organization_prioritized_only.rs.

## Steps to Reproduce

1. Open services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/list_recommendations.rs and read the `matching` filter closure (lines 261-285) and the `list_scope` argument (lines 303-315).
2. Open services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/list_organization_recommendations.rs and read the `matching` filter closure (lines 250-276) and the `list_scope` argument (lines 293-305): the same eight filter tests, the same two inclusive timestamp bounds and the same eight scope pairs in the same order are written out a second time.
3. Compare with src/state.rs:861-901 (`ResourceFilters::keeps` / `::scope`) and src/state.rs:638-754 (`LifecycleWritable` / `apply_lifecycle_update`), where the crate states the equivalent two-level rules exactly once.
4. Change one copy only — e.g. make `checkIdentifier` compare case-insensitively in `ListRecommendations` — and observe that `ListOrganizationRecommendations` keeps the old semantics with no test or type error to catch the divergence.
