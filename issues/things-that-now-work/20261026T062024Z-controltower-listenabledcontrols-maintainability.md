Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: adb8f986b66b684ed38690a8bd355d4f46d0ebf5

# The list-filter matching rule (filter_set/selected) is implemented three times in this crate

_Filed: 2026-10-26T06:20:24Z_
_Operation: ListEnabledControls | Class: maintainability_

> _Note: this issue was identified against the code base at commit `adb8f986b66b684ed38690a8bd355d4f46d0ebf5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The identical filter-matching rule — turn an optional filter list into a `BTreeSet<&str>`, then treat an absent filter as selecting everything — is written out separately in three handler modules of this crate (`list_enabled_controls.rs`, `list_enabled_baselines.rs`, `list_control_operations.rs`), together with the same `type Filter<'a> = Option<BTreeSet<&'a str>>` alias. The three copies are byte-identical in body, so a change to the rule (for example normalising a compared identifier, or switching to case-insensitive matching) has to be made in three places and the copies can silently drift — one list operation would then narrow differently from its siblings for the same filter member.

Lifting `Filter`, `filter_set` and `selected` into one place this crate already owns (for example `crate::state`, which `ListLandingZoneOperations` will need too when it is implemented) leaves every response and every existing test unchanged: the functions are pure and the call sites are identical.

## Evidence

Three copies of the same two functions and the same type alias, all in this crate:

- `services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/list_control_operations.rs:124-137`:
  `type Filter<'a> = Option<::std::collections::BTreeSet<&'a str>>;`
  `fn filter_set(values: Option<&Vec<String>>) -> Filter<'_> { values.map(|values| values.iter().map(String::as_str).collect()) }`
  `fn selected(filter: &Filter<'_>, actual: &str) -> bool { filter.as_ref().is_none_or(|values| values.contains(actual)) }`
- `services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/list_enabled_baselines.rs:144-157`: the same alias and the same two bodies verbatim (`fn filter_set(values: Option<&Vec<String>>) -> Filter<'_> { values.map(|values| values.iter().map(String::as_str).collect()) }`, `fn selected(filter: &Filter<'_>, actual: &str) -> bool { filter.as_ref().is_none_or(|values| values.contains(actual)) }`), plus the extra `selects_an_absent_value` at lines 162-164.
- `services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/list_enabled_controls.rs:136-171`: `fn selected(filter: &Filter<'_>, actual: &str) -> bool` at line 136, `fn selected_optional(...)` at line 145 and `fn filter_set(values: Option<&Vec<String>>) -> Filter<'_>` at line 169 — the third copy.

Existing tests that pin the behaviour the copies must keep: `tests/all/enabled_control_lifecycle.rs:482` `the_listing_is_narrowed_by_target_and_by_every_filter_member`, `tests/all/list_enabled_baselines.rs:304` `the_listing_is_narrowed_by_every_filter_member`, `tests/all/control_operations.rs:276` `the_listing_is_the_log_narrowed_by_every_filter_member`.

## Steps to Reproduce

1. grep -n 'fn filter_set\|fn selected' services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/*.rs — three definitions of `filter_set` and three of `selected` are reported, in list_enabled_controls.rs, list_enabled_baselines.rs and list_control_operations.rs.
2. Compare the bodies: all three `filter_set` bodies are `values.map(|values| values.iter().map(String::as_str).collect())` and all three `selected` bodies are `filter.as_ref().is_none_or(|values| values.contains(actual))`.
3. Change the rule in one module only (e.g. trim the compared value) and observe that the other two list operations keep the old behaviour — nothing in the crate ties the copies together.
