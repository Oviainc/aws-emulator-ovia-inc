Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 50b3c93149d5dec25947e65afb1a4e98fbe7eaad

# UpgradeAccountPlan's free-window refusal varies its message with the stored plan, in an envelope this crate documents as carrying a fixed sentence

_Filed: 2026-11-10T18:38:52Z_
_Operation: UpgradeAccountPlan | Class: error-response_

> _Note: this issue was identified against the code base at commit `50b3c93149d5dec25947e65afb1a4e98fbe7eaad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states ONE rule for the `message` a handler-raised `ValidationException` of this service carries: the sentence states the CONDITION and nothing else, so every request that breaks the rule reads identically. It is written once, on the envelope every handler-worded refusal passes through (`crate::state::stated_validation`), and the crate draws that contrast explicitly for `UpgradeAccountPlan`'s OTHER self-worded refusal: `PAYMENT_INSTRUMENT_VET_FAILURE` is documented as FIXED text that "nothing of the request and nothing in the store varies".

The sibling refusal in the very same handler breaks that rule. The free-window refusal (`target == FREE && !plan.free_window_open()`) interpolates the STORED plan's type and status into the sentence it hands to the same `stated_validation` envelope, so one condition — "this account's one-off free window is no longer open" — reads with two different texts depending on which way the window happens to be closed:

* an account that left the free plan: `[Message = accountPlanType: the FREE plan is a one-off window this account no longer holds (its account plan is PAID and ACTIVE), so it cannot be moved back onto it]`
* an account whose window elapsed: `[Message = ... (its account plan is FREE and EXPIRED) ...]`

Both are reachable over the wire and both are answered by the handler itself (the member is a declared enum value, so the framework's constraint walk does not pre-empt them). A client branching on or re-prompting from the reason therefore sees a reason that varies with something the rule does not distinguish — exactly what the envelope's doc says must not happen. Nothing pins either sentence (the two suites that drive this refusal assert only the status and the error code), so the two halves of one rule can keep drifting.

This is NOT the already-fixed `UpgradeAccountPlan` error-response gap (20261024T152843Z, the `application/x-amz-json-1.0` content-type on pre-handler refusals), and it is not the open `GetFreeTierUsage` echo issue (20261110T072551Z), which is about a different operation's filter branches. The free-window RULE itself was introduced by a fixed issue (20261024T000352Z) and must stay exactly as it is: only the sentence's variable parts are at issue.

## Evidence

The rule, stated once on the envelope every self-worded refusal of this service passes through — `services/service-com-amazonaws-freetier-awsfreetierservice/src/state.rs:702-707`:
```
/// `sentence` states the condition and nothing of the request: a reason string
/// that varied with the input would make a client's branch on it depend on
/// something the rule does not distinguish.
pub fn stated_validation(sentence: &str) -> String {
    format!("[Message = {sentence}]")
}
```

The same rule spelled out for THIS operation's other self-worded refusal — `src/state.rs:713-719`:
```
/// Stated VERBATIM, like [`ACTIVITIES_INVALID_NEXT_TOKEN`] and unlike the
/// sentences [`stated_validation`] wraps: it is the whole `message` of the
/// `ValidationException` (HTTP 400) the refusal carries. It is FIXED text —
/// nothing of the request and nothing in the store varies it — so a caller sees
/// the same words whichever account asked and however often, which is what makes
/// the refusal idempotent.
pub const PAYMENT_INSTRUMENT_VET_FAILURE: &str = "Account plan upgrade failed PI vet failure";
```

And as this crate's own suite states it for handler-checked conditions — `tests/all/validation_message_rules.rs:10-13`:
```
//! 1. a condition a HANDLER checks (the root-operator arity of a `filter`, the number of
//!    match options, a `nextToken` the service did not issue) is stated as ONE fixed
//!    sentence for the whole condition: nothing of the request appears in it, so every
//!    input that breaks the rule reads the same.
```

The code that breaks it — `src/ops/upgrade_account_plan.rs:229-235`:
```
if target == crate::state::PLAN_TYPE_FREE && !plan.free_window_open() {
    return Err(validation(format!(
        "accountPlanType: the FREE plan is a one-off window this account no longer \
         holds (its account plan is {} and {}), so it cannot be moved back onto it",
        plan.plan_type, plan.plan_status
    )));
}
```
handed to the same envelope at `src/ops/upgrade_account_plan.rs:250-255`:
```
fn validation(message: impl AsRef<str>) -> UpgradeAccountPlanError {
    UpgradeAccountPlanErrorModeled::ValidationException(ValidationException {
        message: crate::state::stated_validation(message.as_ref()),
    })
```
while the sibling refusal eight lines above goes out verbatim — `src/ops/upgrade_account_plan.rs:209-213`:
```
if target == crate::state::PLAN_TYPE_PAID && !store.payment_instrument_vetted(account_id) {
    return Err(stated_verbatim(
        crate::state::PAYMENT_INSTRUMENT_VET_FAILURE,
    ));
}
```

That both branches of the condition are reachable, and that the message is pinned by NOTHING:
* `tests/all/account_plan_window.rs:109-180` drives the refusal from a `PAID` plan (`"a paid plan, asked for FREE"`) and from four differently-aged expired `FREE` plans, and `:192-212` asserts only `refused(&router, "UpgradeAccountPlan", &request, 400, "ValidationException")` plus that the stored plan is untouched — no assertion on `message`;
* `tests/all/free_tier.rs:381-405` (`upgrade_account_plan_refuses_a_move_back_to_the_free_plan`) likewise asserts only the status and code;
* a repo-wide grep for the sentence's words (`one-off window`, `its account plan is`) matches no test assertion.

## Steps to Reproduce

1. Boot the emulator with the freetier service mounted and mark the account's payment instrument vetted (as `tests/all/free_tier.rs:64-71`'s `vetted_router()` does), then call UpgradeAccountPlan(accountPlanType="PAID") -> HTTP 200.
2. Call UpgradeAccountPlan(accountPlanType="FREE") and read the message: `[Message = accountPlanType: the FREE plan is a one-off window this account no longer holds (its account plan is PAID and ACTIVE), so it cannot be moved back onto it]` (src/ops/upgrade_account_plan.rs:229-235).
3. On a fresh harness, age the stored free plan past its expiry (the way `tests/all/account_plan_window.rs:95-102` does) and call UpgradeAccountPlan(accountPlanType="FREE"): the same condition is refused with a DIFFERENT sentence, `... (its account plan is FREE and EXPIRED) ...`.
4. In the same harness call UpgradeAccountPlan(accountPlanType="PAID") without a vetted payment instrument: the sibling refusal answers the one fixed sentence `Account plan upgrade failed PI vet failure`, which src/state.rs:713-719 documents as varying with neither the request nor the store.
5. Compare the two against src/state.rs:702-707 — the one envelope both are stated in, whose doc says the sentence states the condition and nothing that varies with the input — and against tests/all/validation_message_rules.rs:10-13: one of this operation's two reachable self-worded refusals contradicts the rule the other obeys.
6. Confirm nothing pins it: re-run tests/all/account_plan_window.rs and tests/all/free_tier.rs after changing the sentence's variable parts; both still pass, since they assert only HTTP 400 and `ValidationException`.
