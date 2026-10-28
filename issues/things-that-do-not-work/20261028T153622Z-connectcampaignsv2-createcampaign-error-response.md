Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 527cd3726300db8090bc233c982f66d05e13948e

# The communicationLimitsList and dailyHours refusals hand-roll their sentence, bypassing the crate's one constraint-violation wording

_Filed: 2026-10-28T15:36:21Z_
_Operation: CreateCampaign | Class: error-response_

> _Note: this issue was identified against the code base at commit `527cd3726300db8090bc233c982f66d05e13948e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` declares exactly one way a handler-side refusal of a modelled constraint is worded: build a `::smithy_server_runtime::Violation` and render it through `aws_smithy_schema_server::constraints::envelope`, "the one place that wording lives — so a handler refusal and a boundary refusal read identically". All three tag checks in that file do so (`tag_keys_violation`, `tag_key_list_violation`, `tag_count_violation`).

The two other handler-side constraint checks in the SAME file do not: `communication_limits_violation` (the `@length` of `CommunicationLimits$communicationLimitsList`) and `communication_time_violation` (the `DayOfWeek` value set of `dailyHours`' keys) each `format!` their own sentence. The result is a sentence the framework never produces: `envelope` prefixes every clause with `"1 validation error detected: "`, and `Violation::message` renders a clause as `Value '<shown>' at '<path>' ...` or `Value null at '<path>' ...` — never the bare `Value at '<member>' ...` the limits check emits. So a client that compares a refusal of `communicationLimitsOverride.allChannelSubtypes.communicationLimitsList`'s `@length` (caught in the handler) with a refusal of any bound the walk does catch gets two differently shaped sentences for the same class of violation, which is exactly what the quoted rule exists to prevent.

The five call sites affected are `CreateCampaign` (both helpers), `UpdateCampaignCommunicationLimits`, `PutInstanceCommunicationLimits` and `UpdateCampaignCommunicationTime`.

## Evidence

The rule, in this crate — src/state.rs:205-213 (doc of `tag_keys_violation`):
```
/// ... and the sentence is built
/// from the framework's own [`::smithy_server_runtime::Violation`] and rendered by
/// `aws_smithy_schema_server::constraints::envelope` — the one place that wording
/// lives — so a handler refusal and a boundary refusal read identically.
```
and it is followed there: src/state.rs:245 `::aws_smithy_schema_server::constraints::envelope(&violations)` (also :278, :298).

The two checks that break it — src/state.rs:352-357:
```
    (count > MAX_COMMUNICATION_LIMITS).then(|| {
        format!(
            "Value at '{member}' failed to satisfy constraint: \
             Member must have length less than or equal to {MAX_COMMUNICATION_LIMITS}"
        )
    })
```
src/state.rs:396-400:
```
            return Some(format!(
                "Value '{day}' at '{member}.{channel}.openHours.dailyHours' failed to \
                 satisfy constraint: Member must satisfy enum value set: [{}]",
                DAY_OF_WEEK_VALUES.join(", ")
            ));
```
What the one wording actually renders — vendor/smithy-schema-server/src/constraints.rs:1332-1338:
```
pub fn envelope(violations: &[Violation]) -> Option<String> {
    match violations.len() {
        0 => None,
        1 => Some(format!(
            "1 validation error detected: {}",
            violations[0].message()
        )),
```
vendor/smithy-schema-server/src/constraints.rs:161-175:
```
    /// The full clause that goes inside the envelope. The ONE place this sentence
    /// is written: every constraint reaches it through [`Violation::requirement`],
    /// so a new constraint cannot drift from the others by spelling its own.
    pub fn message(&self) -> String { ... format!("Value '{shown}' at '{}' failed to satisfy constraint: {requirement}", self.path) ... }
```
The crate's own test states the envelope is universal — tests/all/tag_key_refusal.rs:10-13: "The refusal carries one clause per inadmissible key, naming the key and the requirement it failed, in the validation envelope every constraint refusal in this emulator is worded with", and builds it at tests/all/tag_key_refusal.rs:208-215 (`"1 validation error detected: {}"`).

Call sites: src/ops/create_campaign.rs:184 and :206, src/ops/update_campaign_communication_limits.rs:174, src/ops/put_instance_communication_limits.rs:173, src/ops/update_campaign_communication_time.rs:182.

Test coverage that stays green: the over-long-list and off-catalog-day cases assert only the status and the error code, never the message — tests/all/campaign_communication_limits.rs:494-502 (`assert_eq!(status, 400 ...)` / `error_code(&response).ends_with("ValidationException")`), same shape in tests/all/instance_communication_limits.rs and tests/all/campaign_communication_limits_deletion.rs.

## Steps to Reproduce

1. Onboard a Connect instance and create a campaign in that region.
2. Call `POST /v2/campaigns/{id}/communication-limits` with `communicationLimitsOverride.allChannelSubtypes.communicationLimitsList` carrying three entries (one past the modelled `@length` max of 2).
3. Read the 400 response's `message`: `Value at 'communicationLimitsOverride.allChannelSubtypes.communicationLimitsList' failed to satisfy constraint: Member must have length less than or equal to 2` — no `1 validation error detected: ` envelope, and the `Value at '...'` form `Violation::message` never emits.
4. For comparison, send a tag map with an inadmissible key to `POST /v2/tags/{arn}` and read its message: `1 validation error detected: Value '<key>' at 'tags.<key>.member' failed to satisfy constraint: ...` — the same class of violation, worded differently.
