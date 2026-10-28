Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 74a7ab5ec0f2cdc14f34db1c4c0b190dee7c6aa4

# Drop the handler-side filter-name enum re-check and its hand-copied value set: the generated request walk already refuses the same names

_Filed: 2026-10-28T14:38:41Z_
_Operation: ListAutomationRules | Class: maintainability_

> _Note: this issue was identified against the code base at commit `74a7ab5ec0f2cdc14f34db1c4c0b190dee7c6aa4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "a filter's `name` must be one of the values its filter-name shape declares" is implemented twice in this crate: once by the generated request walk (the framework enforces a legacy `@enum`-constrained string before a handler runs), and once by this crate in three `state.rs` helpers (`unknown_filter_name`, `unknown_event_filter_name`, `unknown_recommended_action_filter_name`) plus a refusal branch in each of the five filtering handlers (ListAutomationRules, ListAutomationEvents, ListAutomationEventSummaries, ListRecommendedActions, ListRecommendedActionSummaries).

Every one of those handler branches is unreachable: the crate's own suite for filter-name refusals pins the BOUNDARY's report (member path `filters.<position>.member.name`, value set in model declaration order) for all five operations at list positions 1 and 2, and the handler's own report can only ever say `filters.member.name` with no position — so no request reaches the handler branch.

The ListAutomationRules copy makes the duplication worse: it does not use the crate's generated, model-derived constant `AUTOMATION_RULE_FILTER_NAME_VALUES` (which `src/ops/types/shared_types.rs:206` already provides, and which its two siblings in the very same module do use); it hand-spells the same six values as a private `AUTOMATION_RULE_FILTER_NAMES`. So one value set lives in the crate twice and the copies can drift from each other and from the model, which is exactly what `state.rs:708-710` states this crate must not do.

Fix: delete the three helpers, the hand-copied constant, `unknown_filter_name_message`, and the five handler branches (or, if the belt-and-braces check is kept, drive all three helpers off the generated constants so no value list is spelled in `state.rs`). Nothing observable changes: in every case a deleted branch would have caught, the boundary answers first with the report the tests already assert.

## Evidence

The crate states that a value outside one of these filter-name sets is reported by the BOUNDARY, not by a handler — `src/state.rs:712-716`:
```
/// Membership of this table is also what separates a named parameter type from
/// the model's legacy `@enum`-constrained STRINGS (`AutomationRuleFilterName`,
/// `AutomationEventFilterName`, `RecommendedActionFilterName`,
/// `SummaryDimensionKey`), which name no parameter type of their own and so keep
/// the generic validation report.
```
and states the no-hand-spelled-value-set convention — `src/state.rs:708-710`:
```
/// Each value set is the GENERATED, model-derived constant for that shape, so no
/// value list is spelled here: the set a request is refused against and the set
/// the refusal enumerates move together with the model.
```

The duplicate value set. Generated, model-derived — `src/ops/types/shared_types.rs:205-206`:
```
/// The values `AutomationRuleFilterName` accepts on the wire.
pub const AUTOMATION_RULE_FILTER_NAME_VALUES: &[&str] = &["AccountId", "Name", "OrganizationConfigurationRuleApplyOrder", "RecommendedActionType", "RuleType", "Status"];
```
Hand-copied, in this crate's state module — `src/state.rs:3507-3516`:
```
/// The `AutomationRuleFilterName` value set, from the Smithy model
/// (`smithy_query(query="shape",shape="AutomationRuleFilterName")`).
const AUTOMATION_RULE_FILTER_NAMES: &[&str] = &[
    "Name",
    "RecommendedActionType",
    "Status",
    "RuleType",
    "OrganizationConfigurationRuleApplyOrder",
    "AccountId",
];
```
Used at `src/state.rs:3416` (`.find(|name| !AUTOMATION_RULE_FILTER_NAMES.contains(name))`) and `src/state.rs:3502` (`AUTOMATION_RULE_FILTER_NAMES.join(", ")`), while the two siblings in the same file use the generated constants: `src/state.rs:2910` (`.find(|name| !AUTOMATION_EVENT_FILTER_NAME_VALUES.contains(name))`) and `src/state.rs:3491` (`.find(|name| !RECOMMENDED_ACTION_FILTER_NAME_VALUES.contains(name))`).

The handler copies are unreachable. The handler branch — `src/ops/list_automation_rules.rs:168-175`:
```
// `Filter$name` is typed as the `AutomationRuleFilterName` value set, but
// a name outside it is refused here too so a filter is never silently
// ignored.
if crate::state::unknown_filter_name(&filters).is_some() {
    return Err(invalid_parameter(
        crate::state::unknown_filter_name_message(),
    ));
}
```
whose body is built with a POSITIONLESS path — `src/state.rs:3497-3505`:
```
pub fn unknown_filter_name_message() -> String {
    constraint_report(
        "filters.member.name",
```
but the suite asserts the boundary's POSITION-BEARING path for positions 1 and 2 — `tests/all/filter_name_refusals.rs:114-120`:
```
fn expected_message(position: usize, accepted: &[&str]) -> String {
    format!(
        "1 validation error detected: Value at 'filters.{position}.member.name' failed to \
         satisfy constraint: Member must satisfy enum value set: [{}]",
        accepted.join(", ")
```
with `for position in [1usize, 2]` and `assert_eq!(message, expected, ...)` (`tests/all/filter_name_refusals.rs:190-209`) over all five filtering operations (`FILTERING_OPERATIONS`, :80-89) and six names including the empty string and a wrong-case spelling (`FILTER_NAMES_TO_SEND`, :98-110). The suite also explains why the generated (sorted) constants are not what the wire message enumerates — `tests/all/filter_name_refusals.rs:41-43`:
```
// The value sets below are the ones the model declares, IN DECLARATION ORDER —
// which is the order a refusal enumerates them in, so the generated constants
// (sorted) cannot be used for the message itself.
```
confirming that the two event/recommended-action handlers, which join the SORTED generated constants into their own refusal (`src/ops/list_automation_events.rs:176-183`, `src/ops/list_recommended_actions.rs:182-189`), could not be producing the message the tests observe either.

Test coverage that must stay green and unchanged: `tests/all/filter_name_refusals.rs::a_filter_name_outside_its_value_set_is_refused_without_being_echoed`, `::paired_listings_refuse_the_same_filter_list_the_same_way`, `::the_value_sets_under_test_are_the_model_s` (which already compares the suite's sets against the generated constants), and `tests/all/enum_parameter_rejection.rs`.

## Steps to Reproduce

1. Read src/ops/types/shared_types.rs:206: the crate already generates AUTOMATION_RULE_FILTER_NAME_VALUES from the model.
2. Read src/state.rs:3507-3516: the same six values are spelled again by hand as AUTOMATION_RULE_FILTER_NAMES, and used at src/state.rs:3416 and :3502 — while the sibling helpers at src/state.rs:2910 and :3491 use the generated constants, and src/state.rs:708-710 states that no value list may be spelled in this module.
3. Read src/ops/list_automation_rules.rs:168-175 together with src/state.rs:3497-3505: the handler's refusal names the member as 'filters.member.name', with no list position.
4. Read tests/all/filter_name_refusals.rs:114-120 and :190-209: the message asserted for every filtering operation is "Value at 'filters.<position>.member.name' ..." for positions 1 and 2, which only the framework's pre-handler validation can produce — so the five handler branches and the three state helpers never run for any request.
5. Conclude that the filter-name enum rule is implemented twice (framework + crate), that one of the crate copies additionally duplicates a generated model-derived value set, and that deleting the crate copies changes no response and no test.
