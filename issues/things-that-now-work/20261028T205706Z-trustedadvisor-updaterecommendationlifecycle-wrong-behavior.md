Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5b375a973f59b1ba0f091f974f00cc96321aba54

# Account-level recommendation operations resolve an identifier naming another account's recommendation, so a lifecycle write lands on a foreign account's resource

_Filed: 2026-10-28T20:57:06Z_
_Operation: UpdateRecommendationLifecycle | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5b375a973f59b1ba0f091f974f00cc96321aba54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states, as a rule over the whole service, that an operation whose input names a resource in ANOTHER account must answer for that resource first, with the 403 `AccessDeniedException` that `crate::state::foreign_resource_refusal` builds, "because no support plan grants an account access to another account's resources". Exactly one handler applies it: `ListRecommendationsForResource`, which calls `foreign_resource_refusal` ahead of even the support-level gate.

The account-level handlers do not. `UpdateRecommendationLifecycle` takes `recommendationIdentifier`, whose modelled pattern is `^arn:[\w-]+:trustedadvisor::\d{12}:recommendation\/[\w-]+$` — i.e. the caller writes the ACCOUNT segment — and the handler goes straight from the support-level gate to `state.recommendations.get_mut(&input.recommendation_identifier)`, with no test of the identifier's account segment, although the crate already owns both the reader (`state::arn_account_id`) and the refusal (`state::foreign_resource_refusal`) for exactly that question. A recommendation stored under another account's ARN is therefore resolved, passes the `priority` type guard (a member account's recommendation is a copy of a prioritized one), and gets its `lifecycleStage`, `updateReason`, `updateReasonCode`, `lastUpdatedAt` and `resolvedAt` overwritten — answered 200. The write is then observable to every caller through `ListOrganizationRecommendationAccounts`, which reports each member account's row from that member recommendation's own stored fields.

This is not a hypothetical store shape: foreign-account member recommendations are exactly what this crate's organization surface is built for (`OrganizationRecommendationEntry::member_recommendation_arns`), and the crate's own suite enrols them into `state.recommendations` / `state.recommendation_resources` under accounts `111122223333`, `999988887777` and `000000000012` — "Written in process because no modelled operation of this API enrols an account … organization membership comes from AWS Organizations, outside this service".

The same missing check sits on the sibling account-level readers that resolve the same identifier: `get_recommendation.rs:227` (`state.recommendations.get(&input.recommendation_identifier)`) and `list_recommendation_resources.rs:211-213` (`state.recommendations.contains_key(&input.recommendation_identifier)`) both serve another account's recommendation and its resources with a 200. One fix — resolving the identifier's account through the helper the crate already has, before the lookup — covers all three sites; with only the seeded store every recommendation ARN carries the emulator account, so no existing response and no existing test changes.

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

The ONE sibling that calls it — src/ops/list_recommendations_for_resource.rs:222-231:
```
        // This operation's input names an ARBITRARY AWS resource, so the resource it
        // targets is authorized FIRST: an ARN in another account is refused for that
        // resource, whatever the caller's support plan, because no plan grants an
        // account access to another account's resources. See
        // [`crate::state::foreign_resource_refusal`].
        if let Some(denied) = crate::state::foreign_resource_refusal(&input.aws_resource_arn) {
```

The handler that skips it and then WRITES — src/ops/update_recommendation_lifecycle.rs:215-232 and 251-260:
```
        let mut state = state.lock();

        let Some(entry) = state
            .recommendations
            .get_mut(&input.recommendation_identifier)
        else {
```
```
        let updated_at = crate::state::now_millis();
        entry.lifecycle_stage = Some(input.lifecycle_stage.clone());
        entry.update_reason = input.update_reason.clone();
        entry.update_reason_code = input.update_reason_code.clone();
        entry.last_updated_at_millis = updated_at;
```
(the only guard between them is the type test at update_recommendation_lifecycle.rs:234-245, `if entry.r#type != crate::state::RECOMMENDATION_TYPE_PRIORITY`, which a member account's prioritized recommendation passes.)

The model lets the caller name any account in that identifier — smithy_query(query="operation", operation="UpdateRecommendationLifecycle"):
```
"recommendationIdentifier": {"target": "AccountRecommendationIdentifier", "httpLabel": true, "required": true, "length": {"min": 20, "max": 200}, "pattern": "^arn:[\\w-]+:trustedadvisor::\\d{12}:recommendation\\/[\\w-]+$"}
```

Foreign-account recommendations really are stored — tests/all/owning_account_from_arn.rs:119-146 (helper `enrol_member_account`) over the accounts at owning_account_from_arn.rs:59-66:
```
fn member_accounts() -> Vec<&'static str> {
    vec![
        engine::EMULATOR_ACCOUNT_ID,
        "111122223333",
        "999988887777",
        "000000000012",
    ]
```
```
    member.arn = member_recommendation_arn(account_id, check_id);
    let member_arn = member.arn.clone();
...
    state.recommendations.insert(member_arn.clone(), member);
```

Test coverage: tests/all/recommendation_loop.rs pins the lifecycle write for the seeded (emulator-account) recommendations, and tests/all/owning_account_from_arn.rs pins the two organization views over the enrolled foreign accounts; neither asserts anything about an account-level call made with a foreign-account identifier, and every seeded recommendation ARN is built by `state::recommendation_arn` through `engine::emulator_arn` (src/state.rs:265-271), so adding the account check leaves all of them answering as they do today.

## Steps to Reproduce

1. Start the emulator with EMULATOR_TRUSTEDADVISOR_SUPPORT_PLAN=business so the support-level gate admits calls (src/state.rs:182).
2. Bring an organization member account into the store the way the crate's own suite does (tests/all/owning_account_from_arn.rs:119-153 `enrol_member_account`): a copy of the seeded `low-utilization-ec2-instances` recommendation and its resources keyed under `arn:aws:trustedadvisor::999988887777:recommendation/low-utilization-ec2-instances-recommendation`, added to that check's organization recommendation's member list.
3. PUT /v1/recommendations/arn%3Aaws%3Atrustedadvisor%3A%3A999988887777%3Arecommendation%2Flow-utilization-ec2-instances-recommendation/lifecycle with body {"lifecycleStage":"resolved","updateReason":"not my account"}.
4. Observe HTTP 200: the handler resolved an identifier naming account 999988887777 and overwrote that account's `lifecycleStage`/`updateReason`/`resolvedAt`, although src/state.rs:62-65 states such a request must be answered for that resource first with the 403 `AccessDeniedException` of src/state.rs:1708-1722 — the refusal GET /v1/recommendations-for-resource/arn%3Aaws%3Aec2%3Aus-east-1%3A999988887777%3Ainstance%2Fi-0abc1de23f4567890 does answer.
5. GET /v1/organization-recommendations/arn%3Aaws%3Atrustedadvisor%3A%3A%3Aorganization-recommendation%2Flow-utilization-ec2-instances-organization-recommendation/accounts and see the foreign account's row now reporting the stage the caller wrote.
6. Read-side symptom of the same missing check: GET /v1/recommendations/arn%3Aaws%3Atrustedadvisor%3A%3A999988887777%3Arecommendation%2Flow-utilization-ec2-instances-recommendation and .../resources both answer 200 with that account's data (src/ops/get_recommendation.rs:227, src/ops/list_recommendation_resources.rs:211-213).
