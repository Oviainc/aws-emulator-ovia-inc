Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5b375a973f59b1ba0f091f974f00cc96321aba54

# ListRecommendations walks the whole recommendation map, so it reports other accounts' recommendations to the caller

_Filed: 2026-10-28T20:57:06Z_
_Operation: ListRecommendations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5b375a973f59b1ba0f091f974f00cc96321aba54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListRecommendations` is the account-level listing: every row it reports carries an `arn` modelled `^arn:[\w-]+:trustedadvisor::\d{12}:recommendation\/[\w-]+$`, i.e. an ARN that names the account the recommendation belongs to. The handler builds its result by iterating `state.recommendations` in full and applying only the request's modelled filters (`type`, `status`, `pillar`, `awsService`, `source`, `checkIdentifier`, the two timestamp bounds); there is no test of the row's owning account, although the crate owns `state::arn_account_id` for reading it off the ARN and `state::foreign_resource_refusal` for the rule that a caller is not given access to another account's resources.

`state.recommendations` is not a per-account map: it is keyed by recommendation ARN across accounts, and the organization surface exists precisely because an organization recommendation aggregates OTHER accounts' recommendations (`OrganizationRecommendationEntry::member_recommendation_arns`, src/state.rs:532-544). Once a member account is enrolled — which the crate's own suite does in process, "the way something outside this API would", because organization membership comes from AWS Organizations rather than from this API — `ListRecommendations` reports that member's recommendation beside the caller's own, so a caller reads rows whose `arn` names an account that is not theirs. The same unscoped walk is what `GetRecommendation`, `ListRecommendationResources` and `UpdateRecommendationLifecycle` resolve a single identifier against (filed separately on the write path).

With only the seeded store every recommendation ARN is built by `state::recommendation_arn` through `engine::emulator_arn`, so it carries the emulator account: restricting the walk to the caller's account changes no seeded response and no existing test.

## Evidence

The unscoped walk — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/list_recommendations.rs:247-267 (every predicate is a request filter; none is the row's account):
```
        let matching: Vec<(String, RecommendationSummary)> = state
            .recommendations
            .iter()
            .filter(|(_, entry)| {
                input.r#type.as_ref().is_none_or(|kind| &entry.r#type == kind)
                    && input.status.as_ref().is_none_or(|status| &entry.status == status)
...
                    && before.is_none_or(|bound| entry.last_updated_at_millis <= bound)
            })
```

The rule the same crate states — src/state.rs:62-65:
```
//! [`seeded_support_plan`]). An operation whose input names an AWS resource in
//! ANOTHER account answers for that resource first
//! ([`foreign_resource_refusal`]): no support plan grants an account access to
//! another account's resources.
```
with the account reader it already has — src/state.rs:1693-1706:
```
/// Every view that reports a stored resource's owning account goes through this,
/// so the account-level and the organization-level view of one stored resource
/// cannot report its account differently.
pub fn arn_account_id(arn: &str) -> Option<String> {
```

That the map holds other accounts' recommendations by design — src/state.rs:532-538:
```
    /// The ARNs of the member accounts' recommendations this organization
    /// recommendation aggregates — keys of
    /// [`recommendations`](ComAmazonawsTrustedadvisorTrustedadvisorState::recommendations).
    /// The emulator presents exactly one account
    /// (`engine::EMULATOR_ACCOUNT_ID`), so the seed records that account's
    /// recommendation for the same check; the member set is a list so an
    /// organization with more accounts needs no second shape.
```
and that enrolling such a member is a supported, exercised state — tests/all/owning_account_from_arn.rs:110-119 and :143:
```
/// Enrol one more member account of the organization: a copy of `check_id`'s
/// seeded recommendation and of every one of its resources, owned by
/// `account_id` and aggregated by that check's organization recommendation.
///
/// Written in process because no modelled operation of this API enrols an
/// account or produces its check results — organization membership comes from
/// AWS Organizations, outside this service — exactly as the sibling suites do
/// it.
```
```
    state.recommendations.insert(member_arn.clone(), member);
```

The model's own scoping of a row's identity — smithy_query(query="shape", shape="Recommendation"): `"arn": {"target": "AccountRecommendationArn", "required": true, "pattern": "^arn:[\\w-]+:trustedadvisor::\\d{12}:recommendation\\/[\\w-]+$"}`.

Test coverage: tests/all/recommendation_loop.rs pins ListRecommendations' filters, order and pagination over the four SEEDED recommendations only, and every seeded ARN is built through `engine::emulator_arn` (src/state.rs:265-271), so scoping the walk to the caller's account leaves those assertions untouched; no test asserts that a foreign-account member recommendation appears in this list.

## Steps to Reproduce

1. Start the emulator with EMULATOR_TRUSTEDADVISOR_SUPPORT_PLAN=business so the support-level gate admits calls (src/state.rs:182).
2. Enrol an organization member account the way tests/all/owning_account_from_arn.rs:119-153 does: insert a copy of the seeded `low-utilization-ec2-instances` recommendation keyed `arn:aws:trustedadvisor::999988887777:recommendation/low-utilization-ec2-instances-recommendation`, plus copies of its resources, and add the member ARN to that check's organization recommendation.
3. GET /v1/recommendations?maxResults=200.
4. Observe the response: `recommendationSummaries` contains a row whose `arn` is `arn:aws:trustedadvisor::999988887777:recommendation/low-utilization-ec2-instances-recommendation` — a recommendation belonging to another account — returned to a caller the crate's own rule (src/state.rs:62-65) says is granted no access to another account's resources.
5. Compare with GET /v1/recommendations-for-resource/arn%3Aaws%3Aec2%3Aus-east-1%3A999988887777%3Ainstance%2Fi-0abc1de23f4567890, which refuses that same account's resource with 403 `AccessDeniedException` via `state::foreign_resource_refusal`.
