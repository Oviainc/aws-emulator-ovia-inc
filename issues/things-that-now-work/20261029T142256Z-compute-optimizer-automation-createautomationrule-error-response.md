Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 0ebb11a4ec2a284a7bba9ad4aa2d81f032f9138d

# CreateAutomationRule words two of its client-error bodies inline, against state.rs's own "every client error this service raises is worded HERE" rule its sibling ceiling refusals follow

_Filed: 2026-10-29T14:22:56Z_
_Operation: CreateAutomationRule | Class: error-response_

> _Note: this issue was identified against the code base at commit `0ebb11a4ec2a284a7bba9ad4aa2d81f032f9138d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state.rs` declares one home for every client-error body of this service, so "two handlers raising the same error cannot word it differently". `CreateAutomationRule` breaks that rule twice, in the same handler:

1. The store-ceiling refusal (`ServiceQuotaExceededException`) is built with an inline `format!` in the handler (`src/ops/create_automation_rule.rs:316-327`), although the crate's two other store-ceiling refusals are functions in `state.rs` beside the bound they state (`tag_limit_exceeded_message`, `account_limit_exceeded_message`). It is the one capacity refusal of this service whose text does not live in the error-wording section.

2. The `IdempotencyTokenInUseException` body is a hand-spelled literal (`src/ops/create_automation_rule.rs:299-308`) sitting directly beside the `IdempotentParameterMismatchException` arm of the SAME `match`, which does read its text from `crate::state::IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE`. One of the two token-outcome messages calls the existing single home; the other does not — and the same literal is then re-spelled in nine sibling handlers, which is exactly the drift the banner comment exists to prevent.

No response byte should change: the fix is to move both strings into `state.rs`'s error-wording section (e.g. a `rule_quota_exceeded_message(account_id)` beside `account_limit_exceeded_message`, and an `IDEMPOTENCY_TOKEN_IN_USE_MESSAGE` constant beside `IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE`) and have every handler read them from there, so the bodies a caller parses stay identical and every existing test keeps passing.

## Evidence

The rule, `src/state.rs:613-625`:
```
// ---------------------------------------------------------------------------
// Error wording
//
// Every client error this service raises is worded HERE, so the body a caller
// parses is built in one place and two handlers raising the same error cannot
// word it differently. Two shapes of rule live here:
//
// * a FIXED message, which the error carries whatever the request named — it
//   must not echo a resource identifier or any other request value;
// * a message BUILT from the request or from the stored resource, which states
//   the fact the caller needs to recover (the member and the constraint it
//   failed, the current revision, the values that are settable).
// ---------------------------------------------------------------------------
```
and the two sibling store-ceiling refusals that follow it — `src/state.rs:689-706`:
```
/// The `InvalidParameterValueException` message for a tag addition that would
/// leave the resource holding more than [`MAX_TAGS_PER_RESOURCE`] tags.
...
pub fn tag_limit_exceeded_message() -> String {
    format!("The request would exceed the limit of {MAX_TAGS_PER_RESOURCE} tags per resource")
}
...
pub fn account_limit_exceeded_message() -> String {
    format!("The request would exceed the limit of {MAX_ENROLLED_ACCOUNTS} accounts")
}
```

The third store-ceiling refusal, worded in the handler instead — `src/ops/create_automation_rule.rs:312-328`:
```
        // The store is in memory and a create is the only operation that grows
        // it, so the per-account rule count is capped and the modelled quota
        // error is what a caller past the cap sees.
        if guard.rules.count_for_account(&account_id) >= crate::state::MAX_RULES_PER_ACCOUNT {
            return Err(
                CreateAutomationRuleErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: Some(format!(
                            "The request would exceed the quota of {} automation rules for \
                             account {account_id}.",
                            crate::state::MAX_RULES_PER_ACCOUNT
                        )),
                    },
                )
                .into(),
            );
        }
```

The two halves of one `match` arm set, one using the single home and one not — `src/ops/create_automation_rule.rs:287-308`:
```
            crate::state::TokenOutcome::ParameterMismatch => {
                return Err(
                    CreateAutomationRuleErrorModeled::IdempotentParameterMismatchException(
                        IdempotentParameterMismatchException {
                            message: Some(
                                crate::state::IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE.into(),
                            ),
...
            crate::state::TokenOutcome::TokenInUse => {
                return Err(
                    CreateAutomationRuleErrorModeled::IdempotencyTokenInUseException(
                        IdempotencyTokenInUseException {
                            message: Some("The specified client token is already in use.".into()),
                        },
```
The same literal is re-spelled in nine sibling handlers: `associate_accounts.rs:283`, `disassociate_accounts.rs:259`, `delete_automation_rule.rs:205`, `rollback_automation_event.rs:245`, `start_automation_event.rs:243`, `tag_resource.rs:239`, `untag_resource.rs:228`, `update_automation_rule.rs:344`, `update_enrollment_configuration.rs:243`.

Test coverage: the refusals are asserted by CODE and status, not by body text — `tests/all/automation_rule_lifecycle.rs:1197-1263` (`ServiceQuotaExceededException` for the per-account rule ceiling) and `:1274-1316` (`IdempotencyTokenInUseException`), with the same token assertion in `tests/all/account_association.rs:611`, `tests/all/account_disassociation.rs:651`, `tests/all/resource_tagging.rs:672` and `tests/all/automation_event_rollback.rs:628` — so relocating the identical strings keeps every one of them green.

## Steps to Reproduce

1. Read src/state.rs:613-625: "Every client error this service raises is worded HERE, so the body a caller parses is built in one place and two handlers raising the same error cannot word it differently."
2. Read src/state.rs:689-706: the tag-per-resource ceiling and the enrolled-account ceiling refusals are both worded there, as functions stating their bound.
3. Read src/ops/create_automation_rule.rs:316-327: the per-account rule ceiling refusal is instead built with an inline format! in the handler — the one capacity refusal of this service worded outside the declared home.
4. Read src/ops/create_automation_rule.rs:287-308: within one match, the ParameterMismatch arm reads crate::state::IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE while the TokenInUse arm hand-spells "The specified client token is already in use."; grep that literal to find nine more copies in sibling handlers.
5. Move both strings into state.rs's error-wording section and have the handlers read them from there; the wire bodies stay byte-identical, so tests/all/automation_rule_lifecycle.rs:1197-1316 and the four sibling token tests still pass.
