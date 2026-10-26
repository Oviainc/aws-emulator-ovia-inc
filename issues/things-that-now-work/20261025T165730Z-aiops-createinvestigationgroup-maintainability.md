Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f3b5c182376fbd512bc0d8c0af2ad2661dfcff47

# The create/update configuration-rejection chain is implemented twice

_Filed: 2026-10-25T16:57:30Z_
_Operation: CreateInvestigationGroup | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f3b5c182376fbd512bc0d8c0af2ad2661dfcff47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The three-rule configuration check a write applies — encryption configuration, then cross-account source role, then the unmodelled collection bounds, in that order and short-circuiting on the first rejection — is spelled out twice in this crate: once in `CreateInvestigationGroup::handle` and once, member-for-member identically, in `UpdateInvestigationGroup::handle`. The individual rules are already single-homed in `crate::state` (which documents each as "Every operation that accepts the structure applies this one rule"), but their COMPOSITION is not, so adding a fourth rule, reordering the chain, or changing which members feed `collection_bounds_rejection` has to be done in two places and the copies can drift (one handler would then refuse an input the other accepts). Single-homing the chain in `crate::state` next to the rules it composes — each handler still wrapping the returned message in its own modelled `ValidationException` — leaves every response byte and every existing test unchanged.

## Evidence

Copy 1 — `services/service-com-amazonaws-aiops-aiops/src/ops/create_investigation_group.rs:230-243`:
```
        if let Some(why) = crate::state::encryption_configuration_rejection(
            input.encryption_configuration.as_ref(),
        )
        .or_else(|| {
            crate::state::cross_account_configurations_rejection(
                input.cross_account_configurations.as_deref(),
            )
        })
        .or_else(|| {
            crate::state::collection_bounds_rejection(
                input.tag_key_boundaries.as_deref(),
                input.chatbot_notification_channel.as_ref(),
            )
        }) {
```
Copy 2 — `src/ops/update_investigation_group.rs:183-202`, the same three calls in the same order with the same arguments:
```
        if let Some(why) = crate::state::encryption_configuration_rejection(
            input.encryption_configuration.as_ref(),
        )
        .or_else(|| {
            crate::state::cross_account_configurations_rejection(
                input.cross_account_configurations.as_deref(),
            )
        })
        .or_else(|| {
            crate::state::collection_bounds_rejection(
                input.tag_key_boundaries.as_deref(),
                input.chatbot_notification_channel.as_ref(),
            )
        }) {
```
The crate's own stated convention for these rules — `src/state.rs:516-517`: "Every operation that accepts the structure applies this one rule, each rendering the refusal as its own modelled `ValidationException`." and `src/state.rs:699-701`: "every operation that stores the list applies it, each rendering the refusal as its own modelled `ValidationException`."
Tests that pin the behaviour both copies must keep: `tests/all/investigation_group_singleton.rs` (`a_policy_document_must_be_json`-style rows plus the cross-account refusal at `:1157-1176` and the encryption rows), `tests/all/investigation_groups.rs:726-744` (update rows), `tests/all/error_message_rules.rs` (refusal wording).

## Steps to Reproduce

1. Open src/ops/create_investigation_group.rs:230-243 and src/ops/update_investigation_group.rs:183-202 side by side: the same three-rule chain appears in both
2. Change the chain in one handler only (e.g. add a rule or swap the order) and observe that the other handler keeps the old behaviour, so the two writes disagree about which input is refused first
3. Run the crate's tests: both copies are exercised (investigation_group_singleton.rs, investigation_groups.rs), so a shared helper must keep the same messages and ordering
