Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0e1d5242aac04ba2a44c3cb18c53433536c8022f

# The "a campaign takes outbound requests only while Running" rule is hand-rolled twice, although the crate centralizes every comparable campaign-state rule on the store

_Filed: 2026-10-28T20:49:15Z_
_Operation: PutOutboundRequestBatch | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0e1d5242aac04ba2a44c3cb18c53433536c8022f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The two batch operations each hand-roll the same gate: resolve the campaign, read the clock-derived state through `StoredCampaign::state_at`, refuse anything other than `Running` with `InvalidCampaignStateException` carrying that state and a sentence built from the SAME format string, and then map `OutboundRequestOutcome::…` arms including an identical `None` -> `InternalServerException` arm with an identical message. `put_profile_outbound_request_batch.rs` even states in its own module doc that this is ONE rule shared with `PutOutboundRequestBatch` ("Both are the rules `PutOutboundRequestBatch` applies to a dial batch; the two differ only in which buffer they write and in the per-request failure catalog they report through"), yet the rule lives in two copies in two handler bodies instead of once on the store.

That is the opposite of how this crate treats every other campaign-state rule: the lifecycle move lives once in `ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::act_on_campaign` with `LifecycleRejected::message`, and the configuration-change gate lives once in `configurable_campaign` with `ConfigurationRejected::message`, each documented as existing precisely "so the four lifecycle operations refuse alike" / "so every campaign-configuration operation refuses alike while each maps the rejection to the error variant its own model declares". The dialability gate is the one campaign-state rule left duplicated, so the two copies can drift: a change to the set of dialable states, or to the refusal sentence, has to be made in two files and nothing fails if only one is edited.

The defect is the duplication only — no response changes. A fix would put the decision and its sentence once beside `act_on_campaign` / `configurable_campaign` (a `dialable_campaign` returning a rejection that carries the judged state and its `message()`), leaving in each handler only the mapping to its own modelled error enum, its own per-request failure catalog, and its own not-found wording, which `state.rs:1086-1089` deliberately keeps different for `PutOutboundRequestBatch` ("`PutOutboundRequestBatch` keeps its own recorded wording, which does name the campaign").

## Evidence

The crate's own stated pattern for exactly this kind of rule — services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/state.rs:1510-1516:

```
/// Why the store refused a lifecycle action on a campaign, as
/// [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::act_on_campaign`]
/// reports it. The stored lifecycle state is unchanged in either case.
///
/// [`Self::message`] carries the sentence the refusal is worded with, so the four
/// lifecycle operations refuse alike while each maps the rejection to the error
/// variant its own model declares.
```
and src/state.rs:1547-1553:
```
/// Why the store refused a configuration change to a campaign, as
/// [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::configurable_campaign`]
/// reports it. Nothing is written in either case.
///
/// [`Self::message`] carries the sentence the refusal is worded with, so every
/// campaign-configuration operation refuses alike while each maps the rejection to the
/// error variant its own model declares.
```

The handler doc that declares this rule shared — src/ops/put_profile_outbound_request_batch.rs:5-10:
```
//! campaign is resolved first — an id the store does not hold is the modelled
//! `ResourceNotFoundException` (HTTP 404) — then its lifecycle state: a campaign takes
//! outbound requests only while it is `Running`, and any other state is the modelled
//! `InvalidCampaignStateException` (HTTP 409) carrying that state. Both are the rules
//! `PutOutboundRequestBatch` applies to a dial batch; the two differ only in which
//! buffer they write and in the per-request failure catalog they report through.
```

Copy 1 — src/ops/put_outbound_request_batch.rs:218-234:
```
        let current = campaign.state_at(now);
        if current != crate::state::CampaignState::Running {
            return Err(
                PutOutboundRequestBatchErrorModeled::InvalidCampaignStateException(
                    InvalidCampaignStateException {
                        state: current.as_wire().to_string(),
                        message: format!(
                            "Campaign {} is {} and takes outbound requests only while Running",
                            input.id,
                            current.as_wire()
                        ),
```

Copy 2 — src/ops/put_profile_outbound_request_batch.rs:209-225, the same computation and the same format string:
```
        let current = campaign.state_at(now);
        if current != crate::state::CampaignState::Running {
            return Err(
                PutProfileOutboundRequestBatchErrorModeled::InvalidCampaignStateException(
                    InvalidCampaignStateException {
                        state: current.as_wire().to_string(),
                        message: format!(
                            "Campaign {} is {} and takes outbound requests only while Running",
                            input.id,
                            current.as_wire()
                        ),
```

The third copied branch, byte-identical in both files — put_outbound_request_batch.rs:290-301 and put_profile_outbound_request_batch.rs:274-285:
```
                // The campaign was resolved above under the guard this walk still
                // holds, so it cannot have gone; answered rather than panicked on.
                None => {
                    return Err(
                        …::InternalServerException(
                            InternalServerException {
                                message: format!(
                                    "campaign {} went missing while its batch was being taken",
                                    input.id
                                ),
```

Test coverage that must stay green and unchanged: tests/all/profile_outbound_requests.rs:505-550 walks the campaign through every state and asserts HTTP 409 with `error_code(&body) == "InvalidCampaignStateException"` and `body["state"] == state`, plus "a refused batch stores nothing"; tests/all/instance_onboarding_tags_and_outbound_requests.rs:654 and :753 assert the same pair (`"InvalidCampaignStateException", Some(&json!(state))`) for the dial batch; tests/all/campaign_schedule_completion.rs:482-617 pins the `Completed`-by-clock refusal; tests/all/outbound_request_expiration.rs and tests/all/campaign_stop.rs:283 exercise the accepted path. None of them pins the refusal MESSAGE, and the message is identical in both copies, so sharing it changes no response.

## Steps to Reproduce

1. Open src/ops/put_outbound_request_batch.rs at line 218 and src/ops/put_profile_outbound_request_batch.rs at line 209: the same `state_at(now) != Running` branch, the same `InvalidCampaignStateException` payload and the same `format!("Campaign {} is {} and takes outbound requests only while Running", …)` sentence appear in both.
2. Compare with src/state.rs:1827 (`act_on_campaign`) and src/state.rs:1870 (`configurable_campaign`): every other campaign-state rule of this crate is decided once on the store, with its sentence on `LifecycleRejected::message` (state.rs:1535) / `ConfigurationRejected::message` (state.rs:1570), and each handler keeps only the mapping to its own modelled error.
3. Edit one copy only — e.g. allow `Paused` to take requests, or reword the sentence in put_outbound_request_batch.rs — and observe that the crate still compiles and the per-operation tests of the other batch operation still pass, so the two copies are free to drift.
4. Confirm the duplication is not forced by the models: the two operations differ only in their error enum, their per-request failure catalog (`BufferLimitExceeded` vs `Conflict`) and their not-found wording (state.rs:1086-1089), none of which is part of the duplicated state gate.
