Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: fffc92c5e8ca342c0e1949ba8f9a858d2cea7932

# CommunicationLimit$frequency / $maxCountPerRecipient @range is never checked, although the same helper hand-checks the list @length behind the same union

_Filed: 2026-10-28T22:29:11Z_
_Operation: UpdateCampaignCommunicationLimits | Class: validation_

> _Note: this issue was identified against the code base at commit `fffc92c5e8ca342c0e1949ba8f9a858d2cea7932`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::communication_limits_violation` exists because the `@length` on `CommunicationLimits$communicationLimitsList` sits behind a union member the centralized constraint walk does not reach. It reads the list's LENGTH and nothing else. The list's elements are `CommunicationLimit` structures whose `maxCountPerRecipient` (`@range min 1`) and `frequency` (`@range 1..30`) are unreached by that same walk and unchecked by the helper, so a limits config carrying `frequency: 0` or `frequency: 500` is accepted at 200 and persisted. `DescribeCampaign` (and `GetInstanceCommunicationLimits`, which shares this helper through `PutInstanceCommunicationLimits`) then serves back a `CommunicationLimit` outside the model's declared range.

The defect is the partial compensation: one constraint behind the union is hand-checked, the two beside it in the same unreached subtree are not.

## Evidence

Smithy, `CommunicationLimits`: `{"type": "union", "members": {"communicationLimitsList": {"target": "CommunicationLimitList", "length": {"max": 2}, "type": "list"}}}`; `CommunicationLimit`: `"maxCountPerRecipient": {"target": "Integer", "range": {"min": 1}, "required": true}, "frequency": {"target": "Integer", "range": {"min": 1, "max": 30}, "required": true}`.

The crate states the unreachability — services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/ops/update_campaign_communication_limits.rs:171-173: "The modelled `@length` of `communicationLimitsList` sits on a union member, which the centralized constraint walk does not reach, so an over-long list is refused here — as a bad request, before anything is looked up or stored."

And the helper it calls reads only the length — src/state.rs:345-357:
```
pub fn communication_limits_violation(
    member: &str,
    limits: Option<&CommunicationLimits>,
) -> Option<String> {
    let count = match limits? {
        CommunicationLimits::communicationLimitsList(list) => list.len(),
    };
    (count > MAX_COMMUNICATION_LIMITS).then(|| {
```
Nothing in the crate inspects a `CommunicationLimit`'s members; the handler then stores the config verbatim — update_campaign_communication_limits.rs:253: `campaign.communication_limits_override = Some(input.communication_limits_override);`

The same helper's doc claims the union is covered — state.rs:339-342: "The one place this service checks a `CommunicationLimits` union against the `@length` its list member carries, so every operation that accepts one (`CreateCampaign`, `UpdateCampaignCommunicationLimits`, `PutInstanceCommunicationLimits`) refuses an over-long list the same way."

Test coverage: tests/all/campaign_communication_limits.rs and tests/all/instance_mutation_audit.rs:72-77 only send in-range rows (`{"maxCountPerRecipient": 3, "frequency": 1, "unit": "DAY"}`), so no test pins out-of-range acceptance.

## Steps to Reproduce

1. Onboard a Connect instance and `CreateCampaign` to get a campaign id.
2. `POST /v2/campaigns/{id}/communication-limits` with `{"communicationLimitsOverride": {"allChannelSubtypes": {"communicationLimitsList": [{"maxCountPerRecipient": 0, "frequency": 500, "unit": "DAY"}]}}}`.
3. Observe HTTP 200 instead of the modelled `ValidationException` (HTTP 400) required by `maxCountPerRecipient`'s `@range min 1` and `frequency`'s `@range 1..30`.
4. `GET /v2/campaigns/{id}` and observe `campaign.communicationLimitsOverride.allChannelSubtypes.communicationLimitsList[0].frequency == 500` — the out-of-range value was persisted and is served back.
5. Repeat against `PUT /v2/connect-instance/{id}/communication-limits` and read it back with `GET .../communication-limits` for the same result.
