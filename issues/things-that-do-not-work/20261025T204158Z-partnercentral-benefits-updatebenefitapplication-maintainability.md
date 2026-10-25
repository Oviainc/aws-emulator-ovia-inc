Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 5c9a4c43e4da79c1a8ad9e3e8eedc72bdd65daf0

# The stale-revision ConflictException message is written out twice, against this crate's own single-construction policy for conflict messages

_Filed: 2026-10-25T20:41:58Z_
_Operation: UpdateBenefitApplication | Class: maintainability_

> _Note: this issue was identified against the code base at commit `5c9a4c43e4da79c1a8ad9e3e8eedc72bdd65daf0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The optimistic-concurrency refusal — 'benefit application {id} is at revision {current}, not {requested}' — is built independently in UpdateBenefitApplication and in AmendBenefitApplication, character for character. The crate already single-homes the other ConflictException message it produces (`crate::state::status_conflict_message`), whose doc states the reason: one construction for every status refusal so a client reads the same words from any of them. The revision refusal is the second conflict message this service produces and it is the one that is duplicated, so the two copies can drift (wording, or which of `revision_text()`/`input.revision` is quoted first) while each handler's own test keeps passing. Folding the two copies into one state helper beside `status_conflict_message` leaves every response byte-identical and every existing test unchanged.

## Evidence

Copy 1 — src/ops/update_benefit_application.rs:304-316:
```
        if application.revision_text() != input.revision {
            return Err(UpdateBenefitApplicationErrorModeled::ConflictException(
                ConflictException {
                    message: format!(
                        "benefit application {} is at revision {}, not {}",
                        application.id,
                        application.revision_text(),
                        input.revision
                    ),
```
Copy 2 — src/ops/amend_benefit_application.rs:533-545:
```
        if application.revision_text() != input.revision {
            return Err(AmendBenefitApplicationErrorModeled::ConflictException(
                ConflictException {
                    message: format!(
                        "benefit application {} is at revision {}, not {}",
                        application.id,
                        application.revision_text(),
                        input.revision
                    ),
```
The policy the crate states for its other conflict message — state.rs:2279-2289:
```
/// The message a request refused because of the application's STATUS carries: the
/// application, the status it is in, and the thing that status does not allow — e.g.
/// ``benefit application benappl-… is IN_REVIEW, so it cannot be updated``.
///
/// One construction for every status refusal this service makes (`submitted`, `recalled`,
/// `updated`, `amended`, `canceled`), so a client reading a `ConflictException` learns
/// the current status from any of them in the same words.
pub fn status_conflict_message(id: &str, status: &str, action: &str) -> String {
```

Tests that cover both copies and must stay green: tests/all/benefit_application_lifecycle.rs:985 `update_refuses_a_revision_that_is_not_the_current_one` (stale, current and non-numeric revisions), tests/all/amend_benefit_application.rs:990 `amend_checks_the_revision_and_is_idempotent_in_its_client_token` (lines 1026-1061), and the stale-revision rows of tests/all/application_status_gates.rs:240-248 and tests/all/amend_terminal_refusal.rs:162-170.

## Steps to Reproduce

1. Open src/ops/update_benefit_application.rs:304-316 and src/ops/amend_benefit_application.rs:533-545: the same `format!("benefit application {} is at revision {}, not {}", …)` appears in both, with the same three arguments.
2. Compare with state.rs:2279-2289, where the sibling ConflictException message (`status_conflict_message`) is written once for all five refusals that use it.
3. Change the wording in only one of the two revision copies: `cargo test -p service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice` still passes for the other operation's suite, so the copies can diverge unnoticed.
