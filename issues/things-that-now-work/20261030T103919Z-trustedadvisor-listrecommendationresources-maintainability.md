Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: bd90440bd38277082eb102c17511325b543b3bd6

# The account-level and organization-level resource views state the same ResourceEntry projection and the same three-filter predicate twice

_Filed: 2026-10-30T10:39:19Z_
_Operation: ListRecommendationResources | Class: maintainability_

> _Note: this issue was identified against the code base at commit `bd90440bd38277082eb102c17511325b543b3bd6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate deliberately states an account-level/organization-level rule ONCE whenever both levels apply it — `state.rs` builds the `LifecycleWritable` seam for exactly that reason, and the crate already centralised the "accountId from an ARN" derivation for the same reason. The resource views are the one remaining pair where that was not done.

`ListRecommendationResources` and `ListOrganizationRecommendationResources` report the SAME stored `crate::state::ResourceEntry`, and each handler writes out the projection of it independently: `resource_summary` and `organization_resource_summary` assign the identical eight members (`id`, `arn`, `aws_resource_id`, `region_code`, `status`, `metadata` — cloned entry by entry the same way — `last_updated_at` through `crate::state::date_time`, `exclusion_status`), differing only in the two members the organization view is modelled to carry differently (`account_id`, and `recommendation_arn` = the organization ARN).

The same duplication is repeated for the FILTER: the `status` / `exclusionStatus` / `regionCode` equality tests with "an absent filter keeps the row" semantics are written once per handler (the organization one additionally testing `affectedAccountId`).

So the invariant the organization handler's own doc asserts — that the two views of one resource "cannot disagree" — is not enforced by anything: it holds only because two separately-written copies happen to agree today. A change to either copy (a differently rendered timestamp, a case-insensitive `regionCode` match, a metadata key rewrite) silently makes the two views of one stored resource disagree. The crate's own test suite asserts that equality member by member, so the drift would show up only as a test failure in the sibling suite, not at the place the copy was edited.

Fix shape: state the shared projection and the shared three-filter predicate once (the organization view layering its two differing members and its extra `affectedAccountId` test on top). Every response byte and every existing test stays unchanged — the two copies are value-identical today.

## Evidence

Copy 1 — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/list_recommendation_resources.rs:106-123:
```
/// The stored resource, as the modelled `RecommendationResourceSummary`.
fn resource_summary(resource: &crate::state::ResourceEntry) -> RecommendationResourceSummary {
    RecommendationResourceSummary {
        id: resource.id.clone(),
        arn: resource.arn.clone(),
        aws_resource_id: resource.aws_resource_id.clone(),
        region_code: resource.region_code.clone(),
        status: resource.status.clone(),
        metadata: resource
            .metadata
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        last_updated_at: crate::state::date_time(resource.last_updated_at_millis),
        exclusion_status: resource.exclusion_status.clone(),
        recommendation_arn: resource.recommendation_arn.clone(),
    }
}
```

Copy 2 — src/ops/list_organization_recommendation_resources.rs:120-140, the same eight assignments with two members replaced:
```
fn organization_resource_summary(
    resource: &crate::state::ResourceEntry,
    organization_recommendation_arn: &str,
) -> OrganizationRecommendationResourceSummary {
    OrganizationRecommendationResourceSummary {
        id: resource.id.clone(),
        arn: resource.arn.clone(),
        aws_resource_id: resource.aws_resource_id.clone(),
        region_code: resource.region_code.clone(),
        status: resource.status.clone(),
        metadata: resource
            .metadata
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        last_updated_at: crate::state::date_time(resource.last_updated_at_millis),
        exclusion_status: resource.exclusion_status.clone(),
        account_id: crate::state::arn_account_id(&resource.arn),
        recommendation_arn: organization_recommendation_arn.to_string(),
    }
}
```

The invariant that second copy's own doc asserts while duplicating it — src/ops/list_organization_recommendation_resources.rs:108-109:
```
/// Every value but two is the resource's own stored one, so the account-level
/// and the organization-level view of a resource cannot disagree. The two:
```

The filter predicate, copy 1 — src/ops/list_recommendation_resources.rs:250-260:
```
            .filter(|(_, resource)| {
                input.status.as_ref().is_none_or(|status| &resource.status == status)
                    && input
                        .exclusion_status
                        .as_ref()
                        .is_none_or(|exclusion| &resource.exclusion_status == exclusion)
                    && input
                        .region_code
                        .as_ref()
                        .is_none_or(|region| &resource.region_code == region)
            })
```
and copy 2 — src/ops/list_organization_recommendation_resources.rs:275-292:
```
            .filter(|(_, summary)| {
                input
                    .status
                    .as_ref()
                    .is_none_or(|status| &summary.status == status)
                    && input
                        .exclusion_status
                        .as_ref()
                        .is_none_or(|exclusion| &summary.exclusion_status == exclusion)
                    && input
                        .region_code
                        .as_ref()
                        .is_none_or(|region| &summary.region_code == region)
                    && input
                        .affected_account_id
                        .as_deref()
                        .is_none_or(|account| summary.account_id.as_deref() == Some(account))
            })
```

The principle this crate states for the other account-level/organization-level pair, and applies there — src/state.rs:596-605:
```
/// A stored recommendation whose lifecycle an `Update…Lifecycle` operation
/// moves: the account-level [`RecommendationEntry`] and the organization-level
/// [`OrganizationRecommendationEntry`].
///
/// The two operations model the same stage and reason-code enums and carry the
/// same "only supports prioritized recommendations" sentence, so they apply ONE
/// rule to two kinds of entry. This is the seam that lets that rule be stated
/// once — by [`lifecycle_update_conflict`] and [`apply_lifecycle_update`] —
/// instead of once per operation, where the account-level and the
/// organization-level lifecycle could drift apart on the same members.
```

Test coverage that pins the two copies' agreement member by member (so the fix must leave it byte-identical) — tests/all/organization_recommendation_resources.rs:339-356:
```
        for (organization, member) in reported.iter().zip(member_reported) {
            for shared in [
                "id",
                "arn",
                "awsResourceId",
                "regionCode",
                "status",
                "exclusionStatus",
                "metadata",
                "lastUpdatedAt",
            ] {
                assert_eq!(
                    organization[shared], member[shared],
```
The filter copies are pinned by `tests/all/organization_recommendation_resources.rs::every_filter_selects_the_member_accounts_resources_it_keeps` (line 435) and by the `ListRecommendationResources` filter rows in `tests/all/recommendation_loop.rs`.

## Steps to Reproduce

1. Open services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/list_recommendation_resources.rs:106-123 and read `resource_summary`.
2. Open src/ops/list_organization_recommendation_resources.rs:120-140 and read `organization_resource_summary`: the same eight `ResourceEntry` members are assigned the same way, including the identical metadata clone loop and the identical `crate::state::date_time(resource.last_updated_at_millis)` conversion.
3. Compare the filter blocks at list_recommendation_resources.rs:250-260 and list_organization_recommendation_resources.rs:275-292: the `status` / `exclusionStatus` / `regionCode` predicate is written twice.
4. Read list_organization_recommendation_resources.rs:108-109, which asserts the two views "cannot disagree" — nothing in the code enforces that; it holds only because the two copies are written identically.
5. Contrast with src/state.rs:596-605, where the crate states and applies the opposite policy for the account-level/organization-level lifecycle write (`LifecycleWritable` + `apply_lifecycle_update`), so one rule cannot drift between the two levels.
6. Edit one copy only (e.g. change `region_code` matching to case-insensitive in the account-level filter) and run `cargo test -p service-com-amazonaws-trustedadvisor-trustedadvisor`: the failure surfaces in the sibling organization suite (tests/all/organization_recommendation_resources.rs:339-356), not at the edited site — the drift the single-statement seam exists to prevent.
