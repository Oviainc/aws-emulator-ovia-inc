Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a13c4bd7833d49550a5f70ddd7b2f501243c6c8c

# Source's union-member ARN constraints are never checked, unlike the sibling union constraints this crate compensates for by hand

_Filed: 2026-10-28T14:37:33Z_
_Operation: UpdateCampaignSource | Class: validation_

> _Note: this issue was identified against the code base at commit `a13c4bd7833d49550a5f70ddd7b2f501243c6c8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateCampaignSource` stores `input.source` into the record with no validation at all. `Source` is a union, and this crate states — twice, in two different handlers and once in `state.rs` — that the framework's centralized constraint walk does NOT reach members behind a union, which is exactly why it hand-rolls `crate::state::communication_limits_violation` (for `CommunicationLimits$communicationLimitsList`'s `@length`) and `crate::state::communication_time_violation` (for the `dailyHours` keys behind `OpenHours`). `Source` gets no such compensating check anywhere in the crate.

The model puts real constraints on that union's members: `Source$customerProfilesSegmentArn` targets `Arn` with `@length(min: 20, max: 500)` and an ARN `@pattern`, and the union's other member `eventTrigger` carries `EventTrigger$customerProfilesDomainArn` with the same `@length` and `@pattern`. Because nothing checks them, a caller can set `source` to `{"customerProfilesSegmentArn": "x"}` (1 character, not an ARN) and the handler answers 200 and persists it. `DescribeCampaign` then projects it back through `StoredCampaign::to_campaign` (`src/state.rs:988`), so the service returns a `Campaign$source` the model's own shape forbids — precisely the failure mode `state.rs:367-369` names for the union it does guard ("without this check a campaign could be stored opening a day the model does not model").

The same unvalidated store happens on the create path (`src/ops/create_campaign.rs:287`, `source: input.source`), which already runs the two sibling union checks immediately above it but not this one.

## Evidence

The model's constraints on the union's members (`smithy_query(query="shape", shape="Source")`):

```
"Source": {"type": "union", "members": {
  "customerProfilesSegmentArn": {"target": "Arn", "length": {"min": 20, "max": 500},
    "pattern": "^arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-z]{2}-[a-z]+-\\d{1,2}:[a-zA-Z0-9-]+:...$"},
  "eventTrigger": "EventTrigger"}}
```
and `smithy_query(query="shape", shape="EventTrigger")`:
```
"customerProfilesDomainArn": {"target": "Arn", "length": {"min": 20, "max": 500}, "pattern": "^arn:...$"}
```

This crate's own statement that such constraints do not reach the handler — services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/ops/update_campaign_communication_limits.rs:171-173:

```
        // The modelled `@length` of `communicationLimitsList` sits on a union member,
        // which the centralized constraint walk does not reach, so an over-long list
        // is refused here — as a bad request, before anything is looked up or stored.
```
and src/state.rs:367-369:
```
/// Those keys sit behind the `OpenHours` union, which the centralized constraint
/// walk does not reach, so without this check a campaign could be stored opening a
/// day the model does not model.
```

The sibling checks that exist (src/state.rs:345 `pub fn communication_limits_violation(`, src/state.rs:371 `pub fn communication_time_violation(`), and the handler that calls both before storing anything (src/ops/create_campaign.rs:184-214).

The path that skips every such check — src/ops/update_campaign_source.rs:230-237, the entire effect of the handler:

```
        // The whole of the operation's effect: the campaign is now sourced from what
        // the request carried, replacing whatever it held. The source is one union, so
        // this is a replacement and not a merge...
        campaign.source = Some(input.source);

        Ok(UpdateCampaignSourceOutput)
```
No call to any `*_violation` helper appears in update_campaign_source.rs, and `grep -n "customer_profiles_segment_arn|Source::" src/` matches nothing outside `src/ops/types/shared_types.rs:177`.

Test coverage: tests/all/campaign_source_update.rs exercises only values INSIDE the bounds — it even pins both ends as accepted ("the 20-character lower bound of Arn's @length", line 160-164, and "the 500-character upper bound of Arn's @length", line 167-169) — and has no case sending a shorter-than-20 or non-ARN value, so nothing pins the current acceptance.

## Steps to Reproduce

1. Register a Connect instance and run `StartInstanceOnboardingJob` for it, waiting out `crate::state::ONBOARDING_JOB_DURATION` so its job reaches SUCCEEDED.
2. `CreateCampaign` with that `connectInstanceId` and a name; keep the returned `id`.
3. `POST /v2/campaigns/{id}/source` with body `{"source": {"customerProfilesSegmentArn": "x"}}` — 1 character, far below `Arn`'s `@length` minimum of 20 and not matching its `@pattern`.
4. Observe HTTP 200 instead of the modelled `ValidationException` (HTTP 400) the shape's constraints call for.
5. `GET /v2/campaigns/{id}` and observe `campaign.source.customerProfilesSegmentArn == "x"` — the service now returns a `Campaign$source` violating `Arn`'s `@length`/`@pattern`.
6. Repeat with `{"source": {"eventTrigger": {"customerProfilesDomainArn": "not-an-arn"}}}`: also 200 and also persisted.
7. Same via the create path: `CreateCampaign` with `"source": {"customerProfilesSegmentArn": "x"}` is accepted, although the two union checks directly above it in create_campaign.rs:184-214 would have refused the analogous out-of-bounds `communicationLimitsOverride` / `communicationTimeConfig`.
