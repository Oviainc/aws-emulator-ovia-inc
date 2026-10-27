Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4

# The campaign lifecycle-transition rule is hand-rolled four times (Start/Pause/Resume/StopCampaign) although the crate already centralizes comparable rules in state.rs

_Filed: 2026-10-27T23:07:10Z_
_Operation: StartCampaign | Class: maintainability_

> _Note: this issue was identified against the code base at commit `c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Start, Pause, Resume and StopCampaign each contain a byte-for-byte identical copy of the same multi-step rule: resolve the campaign in the request's region or answer `ResourceNotFoundException` with `CAMPAIGN_NOT_FOUND_MESSAGE`; read the clock-derived state via `state_at(SystemTime::now())`; ask `CampaignState::after(ACTION)` for the next state; answer `InvalidCampaignStateException` carrying `current.as_wire()` and the message `"A campaign in state {} cannot be {}"` when there is none; otherwise write `campaign.lifecycle_state = next`. Only the `ACTION` constant and the per-operation error enum differ.

Four copies of one rule can drift: a change to the lock span, to which state is judged, to the refusal's `state` member or to the write has to be made four times, and three of the copies silently keeping the old behaviour is exactly the class of bug this crate has already closed elsewhere. The crate demonstrates the alternative twice over in `crate::state`: `rename_campaign` centralizes the lookup + invariant check + write + typed rejection for `UpdateCampaignName`, and `cursor_page` centralizes the paging window for both paged lists. A `CampaignRef`-taking state method returning a typed rejection (as `rename_campaign` does with `RenameRejected`) would let each of the four handlers map the rejection to its own error variant while the rule itself lives once. Every response and every existing test stays unchanged.

## Evidence

Copy 1 — src/ops/start_campaign.rs:179-214:
```
        const ACTION: crate::state::CampaignAction = crate::state::CampaignAction::Start;
...
        let Some(campaign) = store.campaign_mut(&region, &input.id) else {
            return Err(StartCampaignErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException { message: crate::state::CAMPAIGN_NOT_FOUND_MESSAGE.to_string(), ... }
        let current = campaign.state_at(::std::time::SystemTime::now());
        let Some(next) = current.after(ACTION) else {
            return Err(StartCampaignErrorModeled::InvalidCampaignStateException(
                InvalidCampaignStateException {
                    state: current.as_wire().to_string(),
                    message: format!("A campaign in state {} cannot be {}", current.as_wire(), ACTION.verb()),
        campaign.lifecycle_state = next;
```

Copy 2 — src/ops/stop_campaign.rs:181-216 (identical but for `CampaignAction::Stop` and `StopCampaignErrorModeled`):
```
        const ACTION: crate::state::CampaignAction = crate::state::CampaignAction::Stop;
...
        let current = campaign.state_at(::std::time::SystemTime::now());
        let Some(next) = current.after(ACTION) else {
...
        campaign.lifecycle_state = next;
```
Copies 3 and 4 are src/ops/pause_campaign.rs:179-214 and src/ops/resume_campaign.rs:185-220, with `CampaignAction::Pause` / `CampaignAction::Resume`.

The existing centralization precedent in the same crate — src/state.rs:1687-1717:
```
    pub fn rename_campaign(
        &mut self,
        campaign: CampaignRef<'_>,
        name: &str,
    ) -> Result<(), RenameRejected> {
```
and src/state.rs:472-477:
```
pub fn cursor_page<'a, T>(
    items: &'a [T], sequence_of: impl Fn(&T) -> u64, next_token: Option<&str>, max_results: Option<i32>,
) -> Result<CursorPage<'a, T>, UnissuedCursor> {
```
(whose doc at src/state.rs:460-462 states the intent: "The one place the rule the opaque cursor is built around lives ... so every paged list of this service pages identically".)

Tests that pin the behaviour and must stay green unchanged: tests/all/campaign_lifecycle.rs, tests/all/campaign_resume.rs, tests/all/campaign_stop.rs, and tests/all/campaign_schedule_completion.rs — the last asserts all four actions against a closed window, e.g. lines 548-571 (`StartCampaign`/`PauseCampaign`/`ResumeCampaign`/`StopCampaign` rows) and lines 611-625 (HTTP 409, `InvalidCampaignStateException`, `state == "Completed"`).

## Steps to Reproduce

1. Open src/ops/start_campaign.rs:179-214, src/ops/pause_campaign.rs:179-214, src/ops/resume_campaign.rs:185-220 and src/ops/stop_campaign.rs:181-216 side by side.
2. Observe the four blocks are identical apart from the `ACTION` constant and the error-enum name: same region lookup, same `CAMPAIGN_NOT_FOUND_MESSAGE` refusal, same `state_at(now)`, same `after(ACTION)`, same `InvalidCampaignStateException` construction and message format, same `lifecycle_state = next` write.
3. Compare with src/state.rs:1687-1717, where the analogous lookup+check+write rule for UpdateCampaignName lives once as `rename_campaign` returning a typed `RenameRejected`, and with src/state.rs:472 `cursor_page`, documented as "The one place the rule ... lives".
4. Changing the lifecycle rule (for instance which clock-derived state is judged) requires editing four files, and omitting one leaves that operation on the old rule with no test failure.
