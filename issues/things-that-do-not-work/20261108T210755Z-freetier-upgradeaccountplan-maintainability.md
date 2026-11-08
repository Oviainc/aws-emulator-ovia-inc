Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: ae54f48f2b6fca43108843f25838024299d6c3fb

# The "refuse a value outside enum E naming E's value set" rule is implemented twice in this crate, and the handler-side copies have already drifted to the model's declaration order

_Filed: 2026-11-08T21:07:55Z_
_Operation: UpgradeAccountPlan | Class: maintainability_

> _Note: this issue was identified against the code base at commit `ae54f48f2b6fca43108843f25838024299d6c3fb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states ONE rule for how an out-of-enum value is refused: the refusal names the enum's value set in the order the SERVICE publishes for that enum, looked up by value SET through `crate::state::stated_enum_value_set` and applied by `crate::service::validation` to the violations the framework's constraint walk produces. That path is what a client actually reads, and it is pinned by three suites.

But the same logic — "is this value a member of enum E? if not, refuse naming E's value set" — is also hand-written a second time inside the handlers, three times over, and those copies join the value list in the MODEL's declaration order instead of the published one:

* `UpgradeAccountPlan` for `AccountPlanType` (`ACCOUNT_PLAN_TYPE_VALUES.join(", ")` -> `FREE, PAID`, while the crate publishes `PAID, FREE`),
* `ListAccountActivities` for `ActivityStatus` (`ACTIVITY_STATUS_VALUES.join(", ")`, while the crate publishes `IN_PROGRESS, COMPLETED, NOT_STARTED, EXPIRING`),
* `crate::state::language_code_refusal`, reached by BOTH activity readers, for `LanguageCode` (`LANGUAGE_CODE_VALUES.join(", ")`, while the crate publishes `zh-TW, en-US, zh-CN, ...`).

So the two copies of one rule already disagree, which is exactly the drift the single `stated_enum_value_set` lookup exists to prevent: the crate's own doc says the order is a property of the ENUM and holds for "every member targeting one enum — of any operation, at any depth". The handler copies only avoid being visible because the framework's constraint walk refuses an out-of-enum value before `handle` runs (which the handler comments themselves state and the suites pin), so they are dead sentences that nothing can catch when they drift further. Routing these three checks through the one published-order statement (or dropping them, since the walk in front of the handler already enforces enum membership) changes no response and no test.

## Evidence

The ONE rule, with the published order (file:line):

* `src/service/validation.rs:31-35`: "6. an enumerated type's value SET is listed in the order the SERVICE states that enum in, which is not the order the model declares its members in ([`crate::state::stated_enum_value_set`]); applied inside [`framework_requirement`], so one enum reads alike whether its clause is stated at the member itself or nested inside one about the list that holds it;"
* `src/state.rs:775-781`: "An enum is recognised by its value SET rather than by the member that carried the rejected value, so every member targeting one enum \u2014 of any operation, at any depth, whether the clause is stated at the member itself or nested inside one about the list that holds it \u2014 states that enum's set in the one order."
* `src/state.rs:758`: `pub const ACCOUNT_PLAN_TYPE_VALUE_SET: [&str; 2] = ["PAID", "FREE"];` and `src/state.rs:736-739`/`749-750` for the `LanguageCode`/`ActivityStatus` published orders; `src/state.rs:782-787` is the one lookup (`stated_enum_value_set`).

The SECOND copies, in the model's declaration order (`src/ops/types/shared_types.rs:63` is `ACCOUNT_PLAN_TYPE_VALUES: &[&str] = &["FREE", "PAID"]`):

* `src/ops/upgrade_account_plan.rs:187-197`:
```
// `accountPlanType` is an enum member, so the framework refuses a value
// outside it before `handle` runs; checking here as well keeps the
// refusal a 400 `ValidationException` whatever the boundary does with a
// value the enum does not declare.
let target = input.account_plan_type.as_str();
if !ACCOUNT_PLAN_TYPE_VALUES.contains(&target) {
    return Err(validation(format!(
        "accountPlanType: {target} is not an account plan type; expected one of {}",
        ACCOUNT_PLAN_TYPE_VALUES.join(", ")
    )));
}
```
* `src/ops/list_account_activities.rs:183-191`:
```
for status in wanted {
    if !ACTIVITY_STATUS_VALUES.contains(&status.as_str()) {
        return Err(validation(format!(
            "filterActivityStatuses: {status} is not an activity status; expected one \
             of {}",
            ACTIVITY_STATUS_VALUES.join(", ")
        )));
```
* `src/state.rs:814-823`:
```
pub fn language_code_refusal(language_code: Option<&str>) -> Option<String> {
    let code = language_code?;
    if LANGUAGE_CODE_VALUES.contains(&code) {
        return None;
    }
    Some(format!(
        "languageCode: {code} is not a language code; expected one of {}",
        LANGUAGE_CODE_VALUES.join(", ")
    ))
}
```

That the FIRST copy is the one a client reads — i.e. the handler copies are dead and cannot be caught drifting:

* `tests/all/activity_plane_refusals.rs:420-437` asserts the wire message for `UpgradeAccountPlan(accountPlanType="GOLD")` is `"... Member must satisfy enum value set: [{plan_type_set}]"` with `let plan_type_set = ACCOUNT_PLAN_TYPE_VALUE_SET.join(", ");` (the PUBLISHED order), not the handler's sentence;
* same file `:358-383` (`languageCode`, both readers) and `:388-414` (`filterActivityStatuses`, nested at the list) do the same for the other two enums;
* `tests/all/paid_plan_vetting.rs:209-269` (`an_account_plan_type_outside_the_enum_names_the_published_value_set`) pins the same framework-path message over nine submitted values;
* `tests/all/shared_rules.rs:176-245` (`both_activity_readers_check_the_language_code_by_one_rule`) is documented as driving `crate::state::language_code_refusal` (`tests/all/shared_rules.rs:17-22`) but asserts only "the refusal must name the code it refused" plus "must name the modelled code" — both true of the framework sentence — so the handler copy's own wording and ORDER are asserted by nothing.

## Steps to Reproduce

1. Read `src/state.rs:775-787` and `src/service/validation.rs:31-35`: the crate states one rule — an enum's value set is listed in the order the service publishes for that enum, through the single `stated_enum_value_set` lookup.
2. Read `src/ops/upgrade_account_plan.rs:187-197`, `src/ops/list_account_activities.rs:183-191` and `src/state.rs:814-823`: the same "not a member of this enum -> refuse naming the set" rule is written three more times, each joining the model's declaration order (`ACCOUNT_PLAN_TYPE_VALUES` = [FREE, PAID] against the published [PAID, FREE] at `src/state.rs:758`), so the two copies of one rule already disagree.
3. Call `UpgradeAccountPlan` with `accountPlanType="GOLD"` over the wire and observe the message pinned at `tests/all/activity_plane_refusals.rs:429-437`: the set is stated in the published order, i.e. the framework's walk answered and the handler's copy never ran.
4. Confirm no test asserts the handler copies' sentences: `tests/all/shared_rules.rs:208-219` only requires the message to contain the refused code and each modelled value, which the framework sentence satisfies — so a further drift in the three dead copies is unobservable.
5. Fold the three checks onto the one published-order statement (or delete them, the walk in front of `handle` already enforces enum membership) and re-run `tests/all/activity_plane_refusals.rs`, `tests/all/paid_plan_vetting.rs` and `tests/all/shared_rules.rs`: every response and every assertion stays as it is today.
