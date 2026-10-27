Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: b827135cf833dfec17047e08babbbec809d0cc25

# ListOrganizationRecommendationAccounts does not collapse a repeated member recommendation ARN, so a duplicate row shares one pagination key

_Filed: 2026-10-26T08:21:02Z_
_Operation: ListOrganizationRecommendationAccounts | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b827135cf833dfec17047e08babbbec809d0cc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both organization sub-resource listings read the same stored field, `OrganizationRecommendationEntry::member_recommendation_arns`, and both describe it as "an unordered set". `ListOrganizationRecommendationResources` normalises that set before paginating: it sorts AND dedups, with a comment stating the rule ("A member ARN repeated in that set contributes its resources once"). `ListOrganizationRecommendationAccounts` sorts the same set but never dedups, so a member ARN that appears twice produces two IDENTICAL `AccountRecommendationLifecycleSummary` rows that carry the SAME pagination key (the member recommendation ARN).

That key collision is worse than a duplicate row: `crate::state::paginate` resumes a cursor with a strict `key > resume` comparison, so when a page boundary falls on the duplicated key the second copy is filtered out of the next page and the caller following `nextToken` sees the sequence change length between a one-page read and a paged read — breaking the rule the suite states for this operation ("`nextToken` walks every entry exactly once at any page size").

The same un-normalised iteration appears in `ComAmazonawsTrustedadvisorTrustedadvisorState::organization_aggregates_for`, which sums `aggregates_for(member)` once per entry of the list, so a repeated member ARN double-counts its resources in `GetOrganizationRecommendation.resourcesAggregates` while `ListOrganizationRecommendationResources` reports each of those resources once — two responses computed from one store that then disagree. No request can push a duplicate into the member list today (nothing on the wire writes it), so the impact is latent, but the crate already implements the rule in one path and skips it in the sibling paths that read the same field.

## Evidence

Sibling that normalises — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/list_organization_recommendation_resources.rs:258-263,292-293:
```
        // The stored member list is an unordered set, so the page order is
        // resource-ARN order over the union — a stable sequence a cursor can
        // point into, as in every other list of this service. A member ARN
        // repeated in that set contributes its resources once: the keys are
        // resource ARNs, and one resource is one entry of this list.
        let mut matching: Vec<(String, OrganizationRecommendationResourceSummary)> = entry
            .member_recommendation_arns
...
        matching.sort_by(|(left, _), (right, _)| left.cmp(right));
        matching.dedup_by(|(left, _), (right, _)| left == right);
```

This handler, reading the same field, with the same "unordered set" justification but no dedup — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/list_organization_recommendation_accounts.rs:246-263:
```
        // The stored member list is an unordered set, so the page order is
        // member-recommendation-ARN order — a stable sequence a cursor can point
        // into, as in every other list of this service.
        let mut matching: Vec<(String, AccountRecommendationLifecycleSummary)> = entry
            .member_recommendation_arns
            .iter()
...
        matching.sort_by(|(left, _), (right, _)| left.cmp(right));
```

Why a shared key loses a row — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/state.rs:964-967:
```
    let remaining: Vec<(String, T)> = entries
        .into_iter()
        .filter(|(key, _)| after.as_ref().is_none_or(|resume| key > resume))
```

The aggregate path that double-counts the same duplicate — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/state.rs:683-692:
```
        for member in members {
            let aggregates = self.aggregates_for(member);
            total.ok_count += aggregates.ok_count;
```

Test coverage: `tests/all/organization_recommendation_accounts.rs:20-21` states the rule this breaks ("`nextToken` walks every entry exactly once at any page size"), and a repo-wide grep for `dedup|duplicate` under `tests/` shows no test enrols the same member recommendation twice, so no existing test pins the current behaviour either way.

## Steps to Reproduce

1. Start the emulator with an account on a support plan that includes the Trusted Advisor API.
2. Put the same member recommendation ARN into one organization recommendation's `member_recommendation_arns` twice (the only writer of that field; `tests/all/organization_recommendation_accounts.rs::add_member_account` shows the push).
3. GET `/v1/organization-recommendations/{arn}/accounts` and observe two identical summaries for that one member recommendation, whereas GET `.../resources` reports each of its resources once (`matching.dedup_by`, list_organization_recommendation_resources.rs:293).
4. GET `/v1/organization-recommendations/{arn}/accounts?maxResults=N` so the page boundary falls on the duplicated ARN, then follow `nextToken`: the second copy is skipped because `paginate` resumes with `key > resume` (state.rs:966), so the paged walk and the single-page read report different totals.
5. GET `/v1/organization-recommendations/{arn}` and compare `resourcesAggregates` with the rows of `.../resources`: the aggregate counts the duplicated member's resources twice while the resource list reports them once.
