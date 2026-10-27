Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: b4c888ef8ded8aa913bb7b3b9f51a492e4dac1e5

# Fold CreateSlackChannelConfiguration's inline notification-flag default into NotificationFlags::new, which already applies it

_Filed: 2026-10-27T01:55:05Z_
_Operation: CreateSlackChannelConfiguration | Class: maintainability_

> _Note: this issue was identified against the code base at commit `b4c888ef8ded8aa913bb7b3b9f51a492e4dac1e5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The documented default for the three notification flags — an absent flag counts as `false` — is implemented TWICE in this crate: once inline in the `CreateSlackChannelConfiguration` handler, and once in `state::NotificationFlags::new`, whose whole job is that same `unwrap_or(false)`.

The handler computes the three defaults itself, then immediately re-wraps the already-defaulted booleans in `Some(..)` to hand them to the helper that would have defaulted them anyway. The two copies can drift: if the default rule is ever restated in `NotificationFlags::new` (the one place whose doc comment owns it — "A flag that is absent from the scope a clause reads counts as `false` for it"), the create handler's persisted record keeps computing the old one, and the values that go into the store would then disagree with the values the severity/flag rule is evaluated over.

`NotificationFlags`' three fields are `pub` (state.rs:332-335), so the handler can build the flags ONCE from the raw optional inputs and read the defaulted booleans back off it for the record it inserts. That is a mechanical change: the same three booleans are persisted and the same four arguments reach `notification_rule_violation`, so every response byte and every existing test stays unchanged. No behavioral impact — this is LOW.

## Evidence

Copy 1 — services/service-com-amazonaws-supportapp-supportapp/src/ops/create_slack_channel_configuration.rs:214-232:
```
        // The documented default for the three notification flags: "If you don't
        // specify these parameters in your request, they default to false."
        // (`notifyOnCaseSeverity`). ...
        let notify_on_create_or_reopen_case = input.notify_on_create_or_reopen_case.unwrap_or(false);
        let notify_on_add_correspondence_to_case =
            input.notify_on_add_correspondence_to_case.unwrap_or(false);
        let notify_on_resolve_case = input.notify_on_resolve_case.unwrap_or(false);
...
        let flags = crate::state::NotificationFlags::new(
            Some(notify_on_create_or_reopen_case),
            Some(notify_on_add_correspondence_to_case),
            Some(notify_on_resolve_case),
        );
```

Copy 2 — services/service-com-amazonaws-supportapp-supportapp/src/state.rs:338-352:
```
impl NotificationFlags {
    /// The three flags read out of optional members, an absent one counting as
    /// `false`.
    pub fn new(
        notify_on_create_or_reopen_case: Option<bool>,
        notify_on_add_correspondence_to_case: Option<bool>,
        notify_on_resolve_case: Option<bool>,
    ) -> Self {
        Self {
            notify_on_create_or_reopen_case: notify_on_create_or_reopen_case.unwrap_or(false),
            notify_on_add_correspondence_to_case: notify_on_add_correspondence_to_case
                .unwrap_or(false),
            notify_on_resolve_case: notify_on_resolve_case.unwrap_or(false),
        }
    }
```

The fields the handler would read instead are already public — state.rs:331-336:
```
pub struct NotificationFlags {
    pub notify_on_create_or_reopen_case: bool,
    pub notify_on_add_correspondence_to_case: bool,
    pub notify_on_resolve_case: bool,
}
```

Existing test coverage that must stay green unchanged:
- `tests/all/slack_channel_configurations.rs:384` `a_created_configuration_is_reported_by_list_exactly_as_stored`, whose `expected_record` (same file, lines 269-291) independently applies the same default: "the documented default of `false` applied to each absent notification flag" / `json!(self.notify_on_create_or_reopen_case.unwrap_or(false))`.
- `tests/all/update_severity_rule_scope.rs:178` `each_clause_of_the_severity_rule_is_applied_over_its_documented_scope`, which creates every fixture through this handler and sweeps the whole severity/flag space.
- `tests/all/error_message_templates.rs` (constants `ALL_OR_HIGH_RULE` / `NONE_RULE`, lines 50-59) pins the create-side severity refusal messages that `notification_rule_violation` produces from these flags.

## Steps to Reproduce

1. Open services/service-com-amazonaws-supportapp-supportapp/src/ops/create_slack_channel_configuration.rs and read lines 214-232: the handler applies `unwrap_or(false)` to each of the three notification flags, then wraps the results in `Some(..)` for `NotificationFlags::new`.
2. Open services/service-com-amazonaws-supportapp-supportapp/src/state.rs and read lines 338-352: `NotificationFlags::new` applies exactly the same `unwrap_or(false)` to each of the three members.
3. Observe that the same documented rule ("If you don't specify these parameters in your request, they default to false") is therefore computed in two places, and that the create handler's second copy exists only to feed values it had already defaulted into the helper that defaults them.
4. Confirm the consolidation is response-preserving: `NotificationFlags`' three fields are `pub` (state.rs:331-336), so building the flags once from `input.notify_on_*` and reading `flags.notify_on_*` into the inserted `SlackChannelConfig` persists the identical booleans and passes the identical arguments to `notification_rule_violation`.
5. Re-run the crate's test target (`tests/all.rs`): `a_created_configuration_is_reported_by_list_exactly_as_stored`, `each_clause_of_the_severity_rule_is_applied_over_its_documented_scope` and the `error_message_templates` severity tables must all pass unchanged.
