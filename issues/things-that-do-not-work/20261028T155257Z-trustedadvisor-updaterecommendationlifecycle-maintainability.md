Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: dc8acfad318c0cd148284152a9b68532fd3f8c27

# The lifecycle-write rule is implemented twice, once per Update*Lifecycle handler

_Filed: 2026-10-28T15:52:57Z_
_Operation: UpdateRecommendationLifecycle | Class: maintainability_

> _Note: this issue was identified against the code base at commit `dc8acfad318c0cd148284152a9b68532fd3f8c27`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule a lifecycle update applies — refuse a non-`priority` entry with `ConflictException`, then REPLACE `lifecycleStage`, replace both reason members (so an omitted `updateReason`/`updateReasonCode` clears the stored one), stamp `lastUpdatedAt` from the clock, and set `resolvedAt` exactly when the new stage is `resolved` — is written out twice in this crate: once in `src/ops/update_recommendation_lifecycle.rs` over a `state::RecommendationEntry`, and once in `src/ops/update_organization_recommendation_lifecycle.rs` over a `state::OrganizationRecommendationEntry`. The two copies are line-for-line the same computation on five identically named fields.

The crate already recognises the rule is shared: both handlers import the SAME two constants from `crate::state` precisely because the organization-level update "stamps `resolvedAt` on the same stage", and `state.rs` documents `LIFECYCLE_STAGE_RESOLVED` as "Shared by the account-level and the organization-level lifecycle update". What was factored out is the two constants; the RULE that consumes them was copied. So a later change to one copy — e.g. making `resolvedAt` sticky, or keeping a previous `updateReason` when the new request omits it — silently leaves the other copy on the old rule, and the account-level and organization-level lifecycle then disagree about the same five fields.

Fix: let the rule live once (a single `crate::state` helper that takes the new stage, the two reason members and the clock stamp and writes the five fields, applied by both handlers). Every response and every existing test stays byte-for-byte unchanged.

## Evidence

Copy 1 — `services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/update_recommendation_lifecycle.rs:234-260`:
```
        if entry.r#type != crate::state::RECOMMENDATION_TYPE_PRIORITY {
            return Err(UpdateRecommendationLifecycleErrorModeled::ConflictException(
...
        let updated_at = crate::state::now_millis();
        entry.lifecycle_stage = Some(input.lifecycle_stage.clone());
        entry.update_reason = input.update_reason.clone();
        entry.update_reason_code = input.update_reason_code.clone();
        entry.last_updated_at_millis = updated_at;
        // `resolvedAt` is "When the Recommendation was resolved": set it on the
        // update that resolves the recommendation, and clear it on one that moves
        // the recommendation back out of `resolved`.
        entry.resolved_at_millis = (input.lifecycle_stage == LIFECYCLE_STAGE_RESOLVED)
            .then_some(updated_at);
```

Copy 2 — `services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/update_organization_recommendation_lifecycle.rs:244-272`:
```
        if entry.r#type != RECOMMENDATION_TYPE_PRIORITY {
            return Err(
                UpdateOrganizationRecommendationLifecycleErrorModeled::ConflictException(
...
        let updated_at = crate::state::now_millis();
        entry.lifecycle_stage = Some(input.lifecycle_stage.clone());
        entry.update_reason = input.update_reason.clone();
        entry.update_reason_code = input.update_reason_code.clone();
        entry.last_updated_at_millis = updated_at;
        // `resolvedAt` is "When the Recommendation was resolved": set it on the
        // update that resolves the recommendation, and clear it on one that
        // moves the recommendation back out of `resolved`.
        entry.resolved_at_millis =
            (input.lifecycle_stage == LIFECYCLE_STAGE_RESOLVED).then_some(updated_at);
```

The crate states the rule is one rule, having shared only its constants — `services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/update_organization_recommendation_lifecycle.rs:94-98`:
```
// The lifecycle stage that resolves a recommendation, and the recommendation
// type whose lifecycle this operation can move — both shared with the
// account-level `UpdateRecommendationLifecycle`, which models the same enums
// and carries the same "only supports prioritized recommendations" sentence.
use crate::state::{LIFECYCLE_STAGE_RESOLVED, RECOMMENDATION_TYPE_PRIORITY};
```
and `services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/state.rs:218-224`: "the stage on which an update stamps `resolvedAt` ... Shared by the account-level and the organization-level lifecycle update, which model the same stage enum."

Existing tests that pin both copies (so the de-duplication is verifiable without changing any response):
* `services/service-com-amazonaws-trustedadvisor-trustedadvisor/tests/all/recommendation_loop.rs:854` `a_lifecycle_update_persists_and_the_next_read_reports_it`
* `services/service-com-amazonaws-trustedadvisor-trustedadvisor/tests/all/recommendation_loop.rs:976` `a_lifecycle_update_of_a_non_priority_recommendation_conflicts`
* `services/service-com-amazonaws-trustedadvisor-trustedadvisor/tests/all/organization_recommendation_lifecycle.rs:460` `a_lifecycle_update_persists_and_the_next_read_reports_it` (asserted "for every `UpdateRecommendationLifecycleStage` value, every reason code the model declares, and a request that omits the reason members", per that file's header)
* `services/service-com-amazonaws-trustedadvisor-trustedadvisor/tests/all/organization_prioritized_only.rs:166` `every_organization_read_serves_a_prioritized_recommendation_and_no_other`

## Steps to Reproduce

1. Open `services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/update_recommendation_lifecycle.rs` and read lines 234-260.
2. Open `services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/update_organization_recommendation_lifecycle.rs` and read lines 244-272: the same guard and the same five-field write, duplicated.
3. Change the rule in one file only — e.g. drop the `.then_some(updated_at)` clearing so `resolvedAt` survives a move out of `resolved`, or keep `entry.update_reason` when `input.update_reason` is `None`.
4. Run `cargo test -p service-com-amazonaws-trustedadvisor-trustedadvisor`: only that one operation's suite fails, while the sibling operation keeps the old rule — the two copies have drifted with nothing tying them together.
