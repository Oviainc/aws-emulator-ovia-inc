Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a85c524bdc735756e5abfd799d7dcf6de3bb06dc

# StopCampaign accepts a campaign that was never started and moves Initialized -> Stopped

_Filed: 2026-10-24T07:51:25Z_
_Operation: StopCampaign | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a85c524bdc735756e5abfd799d7dcf6de3bb06dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The emulator's campaign state machine treats `Stop` as legal from `Initialized`. `crate::state::CampaignState::after` maps `(Initialized | Running | Paused, Stop) -> Stopped`, so `POST /v2/campaigns/{id}/stop` against a freshly created campaign answers HTTP 200 and the record is written to `Stopped`; a following `GetCampaignState` reports `Stopped` and `PutOutboundRequestBatch` then refuses the campaign for the rest of its life (it only takes requests while `Running`).

AWS refuses to stop a campaign that has never run: the campaign stays `Initialized` and the call is answered with the modeled `InvalidCampaignStateException` (HTTP 409, `state` = `Initialized`), exactly as `PauseCampaign` and `ResumeCampaign` are refused from that state. Expected: HTTP 409 `InvalidCampaignStateException` with `state="Initialized"` and the stored state unchanged. Actual: HTTP 200 and the campaign is now `Stopped`.

The fix belongs in the shared transition table (`CampaignState::after`) so `Stop` is legal only from `Running` and `Paused`; `PauseCampaign`/`StartCampaign` read the same table and must keep their current edges.

## Evidence

Spec/behavior: `StopCampaign` (POST /v2/campaigns/{id}/stop) declares `InvalidCampaignStateException` (HTTP 409) with a required `state` member, the error AWS uses for a lifecycle action that is not legal from the campaign's current state. The behavioral corpus for this service contains the check `connectcampaignsv2.stop-initialized-campaign-rejected` (listed in Issue-Trace-Ids of the open/fixed issue 20261023T134440Z-connectcampaignsv2-behavioral-suite-blocked-and-diverging-...), i.e. recorded AWS behavior is that stopping an `Initialized` campaign is REJECTED — the sibling checks `pause-initialized-campaign-rejected` and `resume-initialized-campaign-rejected` are the same rule for the other two verbs, and the emulator already refuses those two.

Handler/state: services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/state.rs, `CampaignState::after` — `(Self::Initialized | Self::Running | Self::Paused, CampaignAction::Stop) => Some(Self::Stopped)`; src/ops/stop_campaign.rs:179-206 writes `campaign.state = next` whenever `after` returns a state.

Test coverage: NOT asserted either way. tests/all/campaign_lifecycle.rs::get_campaign_state_reports_what_the_lifecycle_calls_wrote walks `pause(409) -> start(200) -> start(409) -> pause(200) -> pause(409) -> start(200) -> stop(200) -> stop(409) -> pause(409) -> start(200)`; every `stop` it issues is from `Running`, so stop-from-Initialized is exercised nowhere and no test locks in the current (divergent) edge. That check in the corpus never ran either — it aborted in setup on the Connect `SearchAvailablePhoneNumbers` 501, which is why this divergence is unreported so far.

## Steps to Reproduce

1. Boot the emulator with the connectcampaignsv2 crate loaded and sign requests for any region.
2. PUT /v2/connect-instance/instance-1/onboarding with {"encryptionConfig":{"enabled":false}} and poll GET /v2/connect-instance/instance-1/onboarding until status is SUCCEEDED.
3. PUT /v2/campaigns with {"name":"lifecycle","connectInstanceId":"instance-1","channelSubtypeConfig":{"telephony":{"outboundMode":{"agentless":{}},"defaultOutboundConfig":{"connectContactFlowId":"flow-0001"}}}} and keep the minted id.
4. GET /v2/campaigns/{id}/state -> "Initialized" (the campaign has never been started).
5. POST /v2/campaigns/{id}/stop -> observe HTTP 200; expected HTTP 409 with error code InvalidCampaignStateException and state="Initialized".
6. GET /v2/campaigns/{id}/state -> observe "Stopped"; expected the state to still be "Initialized" because the refusal must write nothing.
