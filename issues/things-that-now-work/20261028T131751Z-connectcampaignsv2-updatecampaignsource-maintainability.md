Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 930ad079f132337074cf049827d554fb10a3d200

# The "a terminal campaign takes no configuration change" gate is hand-rolled in eight handlers, although the crate already centralizes the sibling lifecycle rule

_Filed: 2026-10-28T13:17:51Z_
_Operation: UpdateCampaignSource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `930ad079f132337074cf049827d554fb10a3d200`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Eight campaign-configuration handlers each re-implement the same three-step rule verbatim: resolve the campaign with `store.campaign_mut(&region, &input.id)` and refuse with `CAMPAIGN_NOT_FOUND_MESSAGE`; read the clock-derived state with `campaign.state_at(SystemTime::now())`; and, when `!current.accepts_configuration_change()`, build an `InvalidCampaignStateException` carrying `current.as_wire()` and the sentence `format!("A campaign in state {} cannot be reconfigured", current.as_wire())`.

The copies are in `update_campaign_source.rs`, `update_campaign_flow_association.rs`, `update_campaign_communication_limits.rs`, `update_campaign_communication_time.rs`, `update_campaign_entry_limits.rs`, `delete_campaign_communication_limits.rs`, `delete_campaign_communication_time.rs` and `delete_campaign_entry_limits.rs`. The predicate itself (`CampaignState::accepts_configuration_change`) is already shared, but the resolve-judge-refuse composition and the refusal sentence are not, so eight copies can drift — one copy could judge `lifecycle_state` instead of `state_at(now)`, or word the sentence differently, without any other copy noticing.

The crate already has exactly the right shape for this, built for the sibling family: `act_on_campaign` plus `LifecycleRejected::message()` put the four lifecycle operations' resolve-judge-refuse rule in one place, leaving each handler only the mapping to its own modelled error variant. The configuration-change gate wants the same treatment: one state.rs entry point that resolves the campaign and judges `accepts_configuration_change`, returning a rejection type whose `message()` is the one place the "cannot be reconfigured" sentence lives. Responses stay byte-identical.

## Evidence

Copy 1 — services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/ops/update_campaign_source.rs:213-228:

```
        let current = campaign.state_at(::std::time::SystemTime::now());
        if !current.accepts_configuration_change() {
            return Err(
                UpdateCampaignSourceErrorModeled::InvalidCampaignStateException(
                    InvalidCampaignStateException {
                        state: current.as_wire().to_string(),
                        message: format!(
                            "A campaign in state {} cannot be reconfigured",
                            current.as_wire()
                        ),
```

Copy 2 — src/ops/delete_campaign_entry_limits.rs:205-220:

```
        let current = campaign.state_at(::std::time::SystemTime::now());
        if !current.accepts_configuration_change() {
            return Err(
                DeleteCampaignEntryLimitsErrorModeled::InvalidCampaignStateException(
                    InvalidCampaignStateException {
                        state: current.as_wire().to_string(),
                        message: format!(
                            "A campaign in state {} cannot be reconfigured",
                            current.as_wire()
                        ),
```

The same block also appears at src/ops/update_campaign_flow_association.rs:210-225, src/ops/update_campaign_communication_limits.rs:226-241, src/ops/update_campaign_communication_time.rs:234-249, src/ops/update_campaign_entry_limits.rs:207-222, src/ops/delete_campaign_communication_limits.rs:238-253 and src/ops/delete_campaign_communication_time.rs:238-253 — eight copies, each preceded by the same `campaign_mut` lookup refusing with `crate::state::CAMPAIGN_NOT_FOUND_MESSAGE`.

The centralization precedent already in this crate — src/state.rs:1533-1544:

```
impl LifecycleRejected {
    /// The sentence this refusal is reported with.
    pub fn message(self) -> String {
        match self {
            Self::NotFound => CAMPAIGN_NOT_FOUND_MESSAGE.to_string(),
            Self::IllegalAction { state, action } => format!(
                "A campaign in state {} cannot be {}",
```
reached through `act_on_campaign` (src/state.rs:1794), whose doc at src/state.rs:1781-1783 states the goal: "The one place the lifecycle rule of this service lives, so `StartCampaign`, `PauseCampaign`, `ResumeCampaign` and `StopCampaign` cannot disagree about it". The shared predicate the eight copies call is src/state.rs:589 (`accepts_configuration_change`), whose own doc says it is "Shared by the configuration operations so two of them cannot disagree about which states are still configurable" — the predicate is shared, the refusal around it is not.

Test coverage that must stay unchanged: tests/all/campaign_source_update.rs:493, tests/all/campaign_entry_limits.rs:564, tests/all/campaign_entry_limits_deletion.rs:526 and tests/all/campaign_schedule_completion.rs:616 all assert the HTTP 409 `InvalidCampaignStateException` these blocks produce; tests/all/campaign_communication_limits.rs:22 and tests/all/campaign_flow_association.rs:21 state the same rule for their operations.

## Steps to Reproduce

1. Open src/ops/update_campaign_source.rs at line 213 and src/ops/delete_campaign_entry_limits.rs at line 205: the `state_at` / `accepts_configuration_change` / `InvalidCampaignStateException` block is byte-identical apart from the error enum name.
2. Grep the crate for `accepts_configuration_change`: eight handler call sites, each followed by its own copy of the refusal construction and the "A campaign in state {} cannot be reconfigured" sentence.
3. Compare with src/state.rs:1794 `act_on_campaign` and src/state.rs:1535 `LifecycleRejected::message()`, where the sibling lifecycle rule and its sentences live exactly once.
4. Change the wording or the judged state in one of the eight copies: no other copy and no shared helper is affected, so the eight can silently diverge.
