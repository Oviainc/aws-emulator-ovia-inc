Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6c829b3943454ada5df5dfe2630a7a4c6694ce1c

# Organization resource list skips the "member recommendation the store does not hold" check its sibling handlers apply, so the two organization views of one aggregate disagree

_Filed: 2026-10-29T06:34:21Z_
_Operation: ListOrganizationRecommendationResources | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6c829b3943454ada5df5dfe2630a7a4c6694ce1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every organization-level view of an organization recommendation is derived from `member_recommendations_of`, and the crate consistently treats a member ARN the store does not actually hold as naming nothing — except in this handler.

`ListOrganizationRecommendationAccounts` resolves each member ARN in `state.recommendations` and SKIPS the member when the lookup misses (`filter_map` + `?`), with a comment stating that rule explicitly. `BatchUpdateRecommendationResourceExclusion` refuses an entry whose owning recommendation is not stored, with the same reasoning. `state::aws_resource_arn_of` likewise resolves the owning recommendation first and answers `None` when it is absent.

`ListOrganizationRecommendationResources` makes no such lookup: it goes straight from the member ARN to `state.resources_of(member_arn)`, which is a pure key-prefix range over the resource map and never consults `state.recommendations`. `organization_aggregates_for` (and `organization_pillar_aggregates_for`, through `cost_totals`, which does resolve the recommendation) go through the same `resources_of`.

Consequence for one organization recommendation whose `member_recommendation_arns` names a member that is not in `recommendations` while that member's resource rows are present: `ListOrganizationRecommendationAccounts` reports NO row for that member, but `ListOrganizationRecommendationResources` reports its resources and `resourcesAggregates` counts them — the accounts view and the resources view of the same aggregate contradict each other, and each reported row carries `accountId` for an account the accounts list denies is a member. The handler's own doc says the resources it reports are "the member accounts' recommendations['] resources, each read off the one stored copy", which presupposes the member recommendation exists.

The missing step is the lookup three sibling paths in this crate already perform; adding it changes no response for a store whose members are all present (the only state the seed and the crate's own member-enrolment helper build), so no existing test moves.

## Evidence

The sibling that applies the check, and states it as a rule — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/list_organization_recommendation_accounts.rs:243-263:
```
        // account wrote is what this list reports for it. A member ARN the store
        // does not hold names no recommendation, and so no account.
...
            .filter_map(|member_arn| {
                let member = state.recommendations.get(member_arn)?;
                Some((member_arn.to_string(), account_lifecycle_summary(member)))
            })
```

The handler that skips it — src/ops/list_organization_recommendation_resources.rs:265-274:
```
        let mut matching: Vec<(String, OrganizationRecommendationResourceSummary)> = state
            .member_recommendations_of(identifier)
            .into_iter()
            .flat_map(|member_arn| state.resources_of(member_arn))
            .map(|(arn, resource)| {
                (
                    arn.clone(),
                    organization_resource_summary(resource, &entry.arn),
                )
            })
```
and the helper it relies on, which deliberately never resolves the recommendation — src/state.rs:643-657:
```
    pub fn resources_of<'a>(
        &'a self,
        recommendation_arn: &'a str,
    ) -> impl Iterator<Item = (&'a String, &'a ResourceEntry)> + 'a {
        // A recommendation ARN outside the modelled form has no prefix — and no
        // resource either, since every stored resource ARN is built from one.
        let prefix = resource_key_prefix(recommendation_arn);
```

The second sibling that refuses exactly this state — src/ops/batch_update_recommendation_resource_exclusion.rs:321-339:
```
            // The type of the recommendation that owns this resource. A resource
            // whose recommendation is not stored belongs to nothing this
            // operation can update, so it is refused like an unknown ARN.
            let owning_type = store
                .recommendations
                .get(&resource.recommendation_arn)
                .map(|recommendation| recommendation.r#type.as_str());
            match owning_type {
                None => {
                    errors.push(entry_error(
                        &exclusion.arn,
                        "ResourceNotFoundException",
```

The third — src/state.rs:783-785:
```
    pub fn aws_resource_arn_of(&self, resource: &ResourceEntry) -> Option<String> {
        let recommendation = self.recommendations.get(&resource.recommendation_arn)?;
```

The aggregate that inherits the same gap — src/state.rs:741-742:
```
        for member in self.member_recommendations_of(organization_recommendation_arn) {
            let aggregates = self.aggregates_for(member);
```

This handler's own doc, which presupposes the member recommendation exists — src/ops/list_organization_recommendation_resources.rs:9-12:
```
//! An organization recommendation holds no resources of its own: it aggregates
//! the member accounts' recommendations for the same check, so the resources it
//! reports are those recommendations' resources, each read off the one stored
//! copy
```

Test coverage: the member-enrolment seam the suite uses inserts the member recommendation, its resources AND the member ARN together — tests/all/organization_recommendation_resources.rs:214-223 (`state.recommendations.insert(member_arn.clone(), member);` … `.member_recommendation_arns.push(member_arn);`) — so every row pinned by `organization_recommendation_resources.rs` and `organization_recommendation_accounts.rs` has its member recommendation stored, and adding the sibling's lookup leaves those responses byte-for-byte unchanged.

## Steps to Reproduce

1. Read src/ops/list_organization_recommendation_accounts.rs:251-264: each member ARN is resolved in `state.recommendations` and a member the store does not hold yields no row.
2. Read src/ops/list_organization_recommendation_resources.rs:265-274: the same member list is handed straight to `state.resources_of(member_arn)`, which (src/state.rs:643-657) only walks the resource map by key prefix and never resolves the recommendation.
3. Take an organization recommendation whose `member_recommendation_arns` names a member ARN that is not a key of `recommendations` while that member's `recommendation_resources` rows exist (the state a member enrolment that inserts resources without the recommendation leaves, cf. the enrolment helper at tests/all/organization_recommendation_resources.rs:184-224).
4. GET /v1/organization-recommendations/{arn}/accounts — the member is absent from `accountRecommendationLifecycleSummaries`.
5. GET /v1/organization-recommendations/{arn}/resources — that same member's resources ARE reported, each carrying its `accountId`, and GET /v1/organization-recommendations/{arn} counts them in `resourcesAggregates` (src/state.rs:741-742): the two organization-level views of one aggregate contradict each other, where BatchUpdateRecommendationResourceExclusion (src/ops/batch_update_recommendation_resource_exclusion.rs:321-339) refuses the very same resources as owned by nothing stored.
