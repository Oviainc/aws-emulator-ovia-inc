Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 69255a4045654719e7d604aff43fb82a4b000a93

# Four handlers' comments claim group_not_found_message is "the ONE sentence every group-scoped refusal of this service uses", which this crate contradicts in five places

_Filed: 2026-10-29T17:05:43Z_
_Operation: AddNotificationChannels | Class: maintainability_

> _Note: this issue was identified against the code base at commit `69255a4045654719e7d604aff43fb82a4b000a93`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate holds its group-not-found RULE in `src/state.rs`, where six different sentences are defined, each with a doc explaining why that operation words its 404 its own way: `group_not_found_message` (the generic one), `policy_group_not_found_message` (GetPolicy), `notification_group_not_found_message` (GetNotificationConfiguration), `update_group_not_found_message` (UpdateProfilingGroup), `profiling_group_record_not_found_message` (Delete/Create) and `agent_group_not_found_message` (ConfigureAgent/PostAgentProfile).

Four handlers paraphrase that rule in their own comments, and all four state it WRONGLY: they call `group_not_found_message` the sentence "every group-scoped operation" / "every group-scoped refusal of this service" uses. `AddNotificationChannels` additionally claims its 404 is "the same 404 the other two notification operations answer" — but one of those two, `GetNotificationConfiguration`, answers a different sentence, as `crate::state` itself documents and as a test pins.

So one rule is stated in five places and the four copies have drifted from the authoritative one. The drift is a regression trap: a maintainer who believes the comment ("this operation cannot word it a second way") and "unifies" the per-operation sentences would reverse the per-operation not-found messages that a FIXED issue (20261023T123333Z, Cause 2) introduced and that `tests/all/refusal_message_rules.rs` / `tests/all/refusal_message_formats.rs` / `tests/all/not_found_surfaces.rs` pin. The fix is to make the four comments point at `crate::state`'s rule instead of restating it incorrectly: no response body, status, message or existing test changes.

## Evidence

The false claim, copy 1 — src/ops/add_notification_channels.rs:161-172:
```
        // The group is looked up FIRST, so a request naming a group the store does not
        // hold answers the same 404 the other two notification operations answer,
        // whatever else the request carries.
        if !guard.groups.contains_key(&input.profiling_group_name) {
            return Err(
                AddNotificationChannelsErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        // The ONE sentence every group-scoped refusal of this service
                        // uses, so this operation cannot word it a second way.
                        message: crate::state::group_not_found_message(
```

Copy 2 — src/ops/remove_notification_channel.rs:158-164:
```
                RemoveNotificationChannelErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        // The ONE sentence every group-scoped refusal of this service
                        // uses, so this operation cannot word it a second way.
                        message: crate::state::group_not_found_message(
```

Copies 3 and 4 — src/ops/put_permission.rs:228-230 and src/ops/remove_permission.rs:208-210 (identical text):
```
                        // The service's own not-found text for an operation ON a group,
                        // as `crate::state` words it for every group-scoped operation.
                        message: crate::state::group_not_found_message(
```

The authoritative rule, which says the opposite — src/state.rs:2057-2067:
```
/// How the NOTIFICATION surface says that the profiling group a request names does not
/// exist: `GetNotificationConfiguration` states the LOOKUP it performed — the account the
/// lookup was scoped to and the name it looked for — rather than the sentence every other
/// group-scoped operation states ([`group_not_found_message`]).
///
/// `AddNotificationChannels` and `RemoveNotificationChannel` keep
/// [`group_not_found_message`] for a group that is not there, ...
pub fn notification_group_not_found_message(name: &str) -> String {
    format!(
        "Profiling group not found with accountId: {}, name: {name}.",
```
and the same contradiction for three more operations — src/state.rs:2036-2039 (`policy_group_not_found_message`: "rather than the sentence every other group-scoped operation states"), src/state.rs:2074-2078 (`update_group_not_found_message`: same clause), src/state.rs:2373-2376 (`agent_group_not_found_message`: "its message is account-qualified ... a different sentence from the one every other operation states ([`group_not_found_message`])").

The handler that proves the notification surface really does word it differently — src/ops/get_notification_configuration.rs:155-165:
```
        let group = guard.groups.get(&input.profiling_group_name).ok_or_else(|| {
            GetNotificationConfigurationErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: crate::state::notification_group_not_found_message(
```

Tests that pin the per-operation sentences and must stay green unchanged: tests/all/refusal_message_rules.rs:575-625 (`the_notification_configuration_states_the_lookup_it_performed`, asserting `format!("Profiling group not found with accountId: {ACCOUNT}, name: {name}.")`), tests/all/refusal_message_rules.rs:467 (`a_group_that_does_not_exist_is_refused_with_the_services_not_found_text`) and :527 (`the_agent_plane_states_its_own_account_qualified_not_found_text`), tests/all/refusal_message_formats.rs:535 (`every_other_group_scoped_operation_states_its_own_not_found_text`), tests/all/not_found_surfaces.rs:137 (`each_group_lookup_states_its_own_not_found_sentence`).

## Steps to Reproduce

1. Read src/ops/add_notification_channels.rs:161-172 — the comment asserts the 404 is "the same 404 the other two notification operations answer" and that `group_not_found_message` is "The ONE sentence every group-scoped refusal of this service uses".
2. Read src/ops/get_notification_configuration.rs:155-165 — the second of those "other two notification operations" answers `crate::state::notification_group_not_found_message` instead, i.e. a different sentence.
3. Read src/state.rs:2036-2039, :2057-2067, :2074-2078 and :2373-2376 — four helpers each documented as the sentence used "rather than the sentence every other group-scoped operation states", so the claim in the handlers is false for GetPolicy, GetNotificationConfiguration, UpdateProfilingGroup and the agent plane.
4. Read src/ops/put_permission.rs:228-230 and src/ops/remove_permission.rs:208-210 — the same false claim, copied verbatim into two more handlers.
5. Replace the four paraphrases with a reference to `crate::state`'s rule (the generic sentence is what these operations keep because their own wording is not recorded, as state.rs:2062-2066 already explains), and re-run tests/all/refusal_message_rules.rs, tests/all/refusal_message_formats.rs, tests/all/not_found_surfaces.rs, tests/all/notification_channels.rs and tests/all/notification_configuration.rs: every status, code and message is unchanged.
