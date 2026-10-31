Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3bf50673ef9b9fde8f395d64d3e48e3a4a38bebc

# Batch exclusion update skips the crate's cross-account resource refusal, answering per-entry "not found" (and writing) for another account's recommendation resource

_Filed: 2026-10-28T23:01:26Z_
_Operation: BatchUpdateRecommendationResourceExclusion | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3bf50673ef9b9fde8f395d64d3e48e3a4a38bebc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states, as a rule over the whole service, that an operation whose input names a resource in ANOTHER account must answer for that resource FIRST with the 403 `AccessDeniedException` that `crate::state::foreign_resource_refusal` builds, "because no support plan grants an account access to another account's resources". `ListRecommendationsForResource` applies that rule (ahead of even the support-level gate); `BatchUpdateRecommendationResourceExclusion` does not, although every one of its entries carries a caller-written account segment: `RecommendationResourceExclusion$arn` is modelled `^arn:[\w-]+:trustedadvisor::\d{12}:recommendation-resource\/[\w-]+\/[\w-]+$`.

The handler goes from the support-level gate straight to `store.recommendation_resources.get_mut(&exclusion.arn)` with no test of the ARN's account, although the crate already owns both the reader (`state::arn_account_id`) and the refusal (`state::foreign_resource_refusal`) for exactly that question. Two consequences:

* Observable today: an entry naming a foreign-account recommendation-resource ARN is answered with a per-entry `ResourceNotFoundException` ("no recommendation resource with the identifier <arn>"), i.e. the service answers about the existence of another account's resource instead of refusing the request for that resource as the crate's rule requires. The answer also discloses existence: an ARN in another account that IS in the store is answered differently from one that is not.
* This is the one WRITE path of the service that takes a caller-supplied cross-account resource ARN. The crate's own suites enrol member accounts' recommendation resources into `state.recommendation_resources` keyed by a foreign-account ARN (`member_resource_arn(account_id, ...)`), so once such an entry's owning recommendation is not `priority` the loop reaches `resource.exclusion_status = ...` / `resource.last_updated_at_millis = ...` and overwrites a foreign account's resource with a 200 and no error entry at all.

One check, through the helper the crate already has, before the per-entry lookup, covers the whole batch; with the seeded store every resource ARN carries the emulator account (`state::recommendation_resource_arn` goes through `engine::emulator_arn`), so no existing response and no existing test changes.

## Evidence

The rule this crate states for every operation — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/state.rs:62-65:
```
//! [`seeded_support_plan`]). An operation whose input names an AWS resource in
//! ANOTHER account answers for that resource first
//! ([`foreign_resource_refusal`]): no support plan grants an account access to
//! another account's resources.
```
and the helper that implements it — src/state.rs:1708-1722:
```
/// The resource-authorization refusal: `Some(..)` when `aws_resource_arn` names a
/// resource in an account other than the caller's, so the request is refused for
/// THAT resource — 403 `AccessDeniedException`, whatever the caller's support
/// plan, because no support plan grants an account access to another account's
/// resources.
...
pub fn foreign_resource_refusal(aws_resource_arn: &str) -> Option<AccessDeniedException> {
    let account = arn_account_id(aws_resource_arn)?;
    (account != ::engine::emulator_account_id()).then(|| AccessDeniedException {
```

The sibling handler that DOES apply it — src/ops/list_recommendations_for_resource.rs:222-231:
```
        // This operation's input names an ARBITRARY AWS resource, so the resource it
        // targets is authorized FIRST: an ARN in another account is refused for that
        // resource, whatever the caller's support plan, because no plan grants an
        // account access to another account's resources. See
        // [`crate::state::foreign_resource_refusal`].
        if let Some(denied) = crate::state::foreign_resource_refusal(&input.aws_resource_arn) {
```

The handler that skips it and then writes — src/ops/batch_update_recommendation_resource_exclusion.rs:282-293 and 329-334:
```
        for exclusion in &input.recommendation_resource_exclusions {
            let Some(resource) = store.recommendation_resources.get_mut(&exclusion.arn) else {
                errors.push(entry_error(
                    &exclusion.arn,
                    "ResourceNotFoundException",
                    format!(
                        "no recommendation resource with the identifier {}",
                        exclusion.arn
                    ),
                ));
                continue;
            };
```
```
            resource.exclusion_status = if exclusion.is_excluded {
                crate::state::EXCLUSION_EXCLUDED.to_string()
            } else {
                crate::state::EXCLUSION_INCLUDED.to_string()
            };
            resource.last_updated_at_millis = updated_at;
```
(the only guards between them are the "is the owning recommendation stored" and "is it prioritized" tests at batch_update_recommendation_resource_exclusion.rs:298-327 — neither looks at the account.)

The model lets the caller name any account in the entry ARN — smithy_query(query="shape", shape="RecommendationResourceExclusion"):
```
"arn": {"target": "RecommendationResourceArn", "required": true, "length": {"min": 20, "max": 2048}, "pattern": "^arn:[\\w-]+:trustedadvisor::\\d{12}:recommendation-resource\\/[\\w-]+\\/[\\w-]+$"}
```

Foreign-account recommendation RESOURCES really are stored by this crate's own suite — tests/all/owning_account_from_arn.rs:133-146:
```
    let copies: Vec<_> = state
        .resources_of(&seeded_arn)
        .map(|(_, resource)| {
            let mut copy = resource.clone();
            copy.arn = member_resource_arn(account_id, check_id, &copy.id);
            copy.recommendation_arn = member_arn.clone();
            copy
        })
        .collect();
...
    for copy in copies {
        state.recommendation_resources.insert(copy.arn.clone(), copy);
    }
```

Test coverage: tests/all/batch_exclusion_unique_arns.rs (`a_batch_of_distinct_resource_arns_still_applies`) and tests/all/recommendation_loop.rs:1017 `an_exclusion_update_persists_and_is_counted` / :1123 `an_exclusion_entry_the_service_cannot_apply_comes_back_per_entry` pin this batch over SEEDED (emulator-account) ARNs only; tests/all/support_level_gate.rs:561 `a_resource_in_another_account_is_refused_naming_that_resource` pins the refusal for the one handler that applies it. No test passes a foreign-account entry to this batch, so adding the account check leaves every existing response unchanged.

## Steps to Reproduce

1. Start the emulator with EMULATOR_TRUSTEDADVISOR_SUPPORT_PLAN=business so the support-level gate admits calls (src/state.rs:182).
2. PUT /v1/batch-update-recommendation-resource-exclusion with body {"recommendationResourceExclusions":[{"arn":"arn:aws:trustedadvisor::999988887777:recommendation-resource/low-utilization-ec2-instances-recommendation/res-ec2-0001","isExcluded":true}]}.
3. Observe HTTP 200 whose single error entry is errorCode ResourceNotFoundException with "no recommendation resource with the identifier arn:aws:trustedadvisor::999988887777:..." — the service answered about another account's resource instead of refusing the request for that resource with the 403 AccessDeniedException src/state.rs:62-65 and src/state.rs:1708-1722 require (and which GET /v1/recommendations-for-resource/arn%3Aaws%3Aec2%3Aus-east-1%3A999988887777%3Ainstance%2Fi-0abc1de23f4567890 does answer).
4. Now enrol a member account's resources the way tests/all/owning_account_from_arn.rs:119-153 does, for a check whose recommendation type is `standard` (e.g. the seeded EBS-volume check, src/state.rs:1284), so state.recommendation_resources holds ARNs under account 999988887777 whose owning recommendation is not prioritized.
5. Repeat the same PUT against one of those foreign ARNs and observe HTTP 200 with an EMPTY error list: the handler wrote exclusion_status and last_updated_at_millis onto another account's resource (batch_update_recommendation_resource_exclusion.rs:329-334), which GET /v1/recommendations/<that account's recommendation ARN>/resources then reports.
