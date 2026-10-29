Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 909749a7cea0ac224aeb09b9b62bd72ae5efa8b6

# The AWS resource ARN a stored resource is matched by stamps the caller's own account onto every account's resources, so ListRecommendationsForResource reports other accounts' findings

_Filed: 2026-10-28T18:43:43Z_
_Operation: ListRecommendationsForResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `909749a7cea0ac224aeb09b9b62bd72ae5efa8b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListRecommendationsForResource` resolves the request's `awsResourceArn` by DERIVING, for every stored recommendation resource, the ARN of the AWS resource it flagged (`state::aws_resource_arn_of` -> `state::aws_resource_arn`). That derivation builds the ARN from the resource's own `regionCode` and `awsResourceId` but hard-codes the account segment to the emulator's canonical account id, instead of the owning account of the resource's OWN ARN (`ResourceEntry::arn`, whose account segment is exactly what the crate's `arn_account_id` exists to read).

Two things follow once an organization has more than one member account — a configuration this crate explicitly supports and exercises (`OrganizationRecommendationEntry::member_recommendation_arns`: "the member set is a list so an organization with more accounts needs no second shape", state.rs:537-538; `tests/all/owning_account_from_arn.rs::enrol_member_account` stores member resources under `arn:aws:trustedadvisor::<other account>:recommendation-resource/...` while keeping the clone's `aws_resource_id` and `region_code`):

1. The derived ARNs COLLIDE: the emulator's own resource and every member account's copy of it derive the identical `arn:aws:ec2:<region>:<emulator account>:instance/<id>`. One GET of that ARN therefore answers one `RecommendationForResourceSummary` per account, three of which describe resources the caller does not own, each with `recommendationArn` pointing at another account's recommendation. `RecommendationForResourceSummary` models no `accountId` member, so nothing in the response says which account a row came from.
2. A resource genuinely owned by a member account is unaddressable by its true ARN: that ARN carries the member's account id, which `foreign_resource_refusal` refuses with 403 before the lookup — so the only ARN that reaches it is the caller's own, which is the leak above.

This contradicts the rule the crate states for exactly this question (derive a stored resource's owning account from that resource's own ARN, so no two views can disagree) and the rule the handler's own first guard states ("no support plan grants an account access to another account's resources"): the handler refuses an ARN in another account and then serves another account's resources under the caller's ARN. Fixing the derivation changes no existing response, because every SEEDED resource is in the emulator's account.

## Evidence

The derivation stamps the emulator's account on every resource — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/state.rs:313-315 and :316-328:
```
/// type's ARN form, so it reports no ARN for it rather than guessing one that a
/// caller could not have formed. The account segment is the emulator's
/// canonical account id, and the Region segment is the resource's own
/// `regionCode` — an AWS resource ARN is Regional.
pub fn aws_resource_arn(
    resource_type: &str,
    region_code: &str,
    aws_resource_id: &str,
) -> Option<String> {
...
    Some(::engine::emulator_arn(service, region_code, &resource))
}
```
and the only caller passes nothing that identifies the owner — src/state.rs:772-782:
```
    pub fn aws_resource_arn_of(&self, resource: &ResourceEntry) -> Option<String> {
        let recommendation = self.recommendations.get(&resource.recommendation_arn)?;
        let check = self.check_of(recommendation)?;
...
        aws_resource_arn(resource_type, &resource.region_code, &resource.aws_resource_id)
    }
```

The rule this crate states for a stored resource's owning account — src/state.rs:1693-1706:
```
/// The owning account a modelled `accountId` member reports for a resource,
/// derived from that resource's own ARN: its account segment, owned, ...
/// Every view that reports a stored resource's owning account goes through this,
/// so the account-level and the organization-level view of one stored resource
/// cannot report its account differently.
pub fn arn_account_id(arn: &str) -> Option<String> {
```

The guard the same handler opens with, and the loop that then ignores ownership — src/ops/list_recommendations_for_resource.rs:222-231:
```
        // This operation's input names an ARBITRARY AWS resource, so the resource it
        // targets is authorized FIRST: an ARN in another account is refused for that
        // resource, whatever the caller's support plan, because no plan grants an
        // account access to another account's resources. See
        // [`crate::state::foreign_resource_refusal`].
        if let Some(denied) = crate::state::foreign_resource_refusal(&input.aws_resource_arn) {
```
and :258-266 (every stored resource of every account is a candidate, matched only on the fabricated ARN):
```
        let matching: Vec<(String, RecommendationForResourceSummary)> = state
            .recommendation_resources
            .iter()
            .filter_map(|(key, resource)| {
                let aws_resource_arn = state.aws_resource_arn_of(resource)?;
                if aws_resource_arn != input.aws_resource_arn {
                    return None;
                }
                let recommendation = state.recommendations.get(&resource.recommendation_arn)?;
```

The multi-account store this reaches is the crate's own supported shape — tests/all/owning_account_from_arn.rs:110-141:
```
/// Enrol one more member account of the organization: a copy of `check_id`'s
/// seeded recommendation and of every one of its resources, owned by
/// `account_id` ...
            let mut copy = resource.clone();
            copy.arn = member_resource_arn(account_id, check_id, &copy.id);
            copy.recommendation_arn = member_arn.clone();
```
(`aws_resource_id` and `region_code` are carried over unchanged, so all four copies derive one ARN.)

Test coverage: tests/all/recommendations_for_resource.rs:66-76 pins the derived ARN with `engine::EMULATOR_ACCOUNT_ID` for the SEEDED resources only ("Source: the `SEED` table in `src/state.rs`"), i.e. for resources whose own ARN already carries that account — so deriving the account from `ResourceEntry::arn` leaves that suite, and every other existing assertion, unchanged. tests/all/support_level_gate.rs is the suite that pins "the refusal of a resource in another account" (recommendations_for_resource.rs:28-30), the rule this leak goes around.

## Steps to Reproduce

1. Start the emulator with EMULATOR_TRUSTEDADVISOR_SUPPORT_PLAN=business so the support-level gate admits calls (src/state.rs:182).
2. Build the multi-account organization this crate supports, as tests/all/owning_account_from_arn.rs:119-153 does: for account 111122223333, insert a copy of the seeded `low-utilization-ec2-instances-recommendation` under `arn:aws:trustedadvisor::111122223333:recommendation/...` plus copies of its three resources under `arn:aws:trustedadvisor::111122223333:recommendation-resource/...` (same `awsResourceId`/`regionCode`), and push the member ARN onto the organization recommendation's `member_recommendation_arns`.
3. GET /v1/recommendations-for-resource/arn%3Aaws%3Aec2%3Aus-east-1%3A<emulator-account>%3Ainstance%2Fi-0abc1de23f4567890 — an ARN in the CALLER's account, so `foreign_resource_refusal` admits it.
4. Observe two `recommendationForResourceSummaries` rows instead of one: the second carries `recommendationArn` = `arn:aws:trustedadvisor::111122223333:recommendation/low-utilization-ec2-instances-recommendation`, i.e. another account's finding, reported under the caller's own resource ARN and with no member in the shape to disclose the owner.
5. GET the same path with the member's true ARN (`...:111122223333:instance/i-0abc1de23f4567890`) and get 403 AccessDeniedException "Access denied to resource: ..." (src/state.rs:1717-1721) — the account whose resource it is cannot be named, while the account it is not leaks it.
