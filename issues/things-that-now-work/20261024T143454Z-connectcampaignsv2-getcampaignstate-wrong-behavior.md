Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# A campaign never reaches Completed: GetCampaignState reports Running forever past the schedule's endTime

_Filed: 2026-10-24T14:34:54Z_
_Operation: GetCampaignState | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The campaign lifecycle the emulator implements has no path into the `Completed` (or `Failed`) states that `CampaignState` declares. `crate::state::CampaignState::after` is the ONLY writer of a campaign's state, and its transition table produces only `Running`, `Paused` and `Stopped`; every other reader (`GetCampaignState`, and `accepts_configuration_change` on the Update*/Delete* config operations) reads the stored value verbatim. Nothing consults the campaign's stored `schedule`.

Expected: a campaign's state is owed to the clock as well as to the lifecycle calls — once the campaign has been started and its `schedule.endTime` has passed, the campaign has finished its run and `GET /v2/campaigns/{id}/state` reports `Completed`, which is the terminal state a client polls for before tearing the campaign down (and which makes the `Completed`/`Failed` arms of `CampaignState::after`'s terminal-state guard, and the `InvalidCampaignStateException` the config operations raise for a terminal campaign, reachable at all).

Actual: the campaign stays in whatever state the last `Start`/`Pause`/`Stop` call wrote — a started campaign whose schedule window closed hours ago still answers `"Running"`, still accepts `PutOutboundRequestBatch` (which only requires `Running`), and still accepts every configuration change, while the two terminal states can never be observed from any sequence of calls.

The crate already has the right pattern for a clock-owed transition one store over: `StoredInstanceOnboarding::status_at(now)` derives `IN_PROGRESS`/`SUCCEEDED` from a stored due instant on every read, so `GetInstanceOnboardingJobStatus` reports a status nothing had to write. The campaign store does not do the same for the schedule it already persists.

## Evidence

Spec: `CampaignState` (com.amazonaws.connectcampaignsv2#CampaignState) declares `Initialized | Running | Paused | Stopped | Failed | Completed` — `GetCampaignStateResponse$state` targets it, so `Completed` is a value AWS is modelled to return and a client is modelled to branch on. `Schedule` is persisted per campaign with `startTime` and `endTime` both `@required`, i.e. the service knows exactly when a campaign's run window closes; a campaign that has run to the end of its schedule is `Completed` in Connect outbound campaigns, which is why `InvalidCampaignStateException` ("conflict in the current state of the campaign") exists on every configuration operation.

Handler/state: src/state.rs — `CampaignState::after` maps only `(Initialized|Paused|Stopped, Start) -> Running`, `(Running, Pause) -> Paused`, `(Paused, Resume) -> Running`, `(Initialized|Running|Paused, Stop) -> Stopped`; `Completed`/`Failed` appear only in the guards `!matches!(self, Self::Completed | Self::Failed)` and are never constructed anywhere in the crate. `StoredCampaign.schedule` is stored by src/ops/create_campaign.rs and projected by `to_campaign`/`to_summary`, but no code reads it to decide anything. src/ops/get_campaign_state.rs:164-166 returns `stored.state.as_wire()` with no clock input (`_ctx`, no `now`).

Test coverage: not asserted. tests/all/campaign_lifecycle.rs only walks the Start/Pause/Stop edges the table already has, and tests/all/campaign_resume.rs enumerates "every reachable state" — by construction that excludes `Completed`/`Failed`, so no test locks in the current behaviour and none covers schedule-driven completion.

## Steps to Reproduce

1. Boot the emulator with the connectcampaignsv2 crate loaded and sign requests for one region.
2. PUT /v2/connect-instance/instance-1/onboarding with {"encryptionConfig":{"enabled":false}} and poll GET /v2/connect-instance/instance-1/onboarding until status is SUCCEEDED.
3. PUT /v2/campaigns with {"name":"ends-soon","connectInstanceId":"instance-1","channelSubtypeConfig":{"telephony":{"outboundMode":{"agentless":{}},"defaultOutboundConfig":{"connectContactFlowId":"flow-0001"}}},"schedule":{"startTime":"<now>","endTime":"<now + 1 minute>"}} and keep the minted id.
4. POST /v2/campaigns/{id}/start -> 200; GET /v2/campaigns/{id}/state -> "Running" (correct while the window is open).
5. Wait until the schedule's endTime has passed, then GET /v2/campaigns/{id}/state -> observe "Running"; expected "Completed" once the campaign's run window has closed.
6. PUT /v2/campaigns/{id}/outbound-requests with one valid telephony request -> observe HTTP 200 and a minted dial id for a campaign whose schedule ended; expected the campaign to be past dialling (409 InvalidCampaignStateException with state="Completed").
7. POST /v2/campaigns/{id}/communication-limits with a valid config -> observe HTTP 200; expected 409 InvalidCampaignStateException, the terminal-state refusal the handler already implements but can never reach.
