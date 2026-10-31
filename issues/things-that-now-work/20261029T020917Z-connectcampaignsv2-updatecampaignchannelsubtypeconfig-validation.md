Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: eb1bf6ad15d70a5e618c7ac6aaae48664667c1d3

# channelSubtypeConfig is stored with no check of the constraints behind the outboundMode/PacingStrategy unions, although the crate states such constraints never reach a handler and its own test asserts seven of those refusals

_Filed: 2026-10-29T02:09:17Z_
_Operation: UpdateCampaignChannelSubtypeConfig | Class: validation_

> _Note: this issue was identified against the code base at commit `eb1bf6ad15d70a5e618c7ac6aaae48664667c1d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateCampaignChannelSubtypeConfig` writes the request's `channelSubtypeConfig` onto the record with no validation at all, and `CreateCampaign` stores the same member the same way after running three sibling union checks right above it. `ChannelSubtypeConfig` carries real constraints BEHIND a union: each channel block's `outboundMode` is a union (`TelephonyOutboundMode`, `SmsOutboundMode`, `EmailOutboundMode`, `WhatsAppOutboundMode`) whose variants carry `bandwidthAllocation` (`@range 0..2`), `PreviewConfig$timeoutConfig$durationInSeconds` (`@range 1..300`), `PredictiveConfig$pacingStrategies` (`@length 1..1`), `agentActions` (the `AgentAction` catalog, `DISCARD` only) and, one union deeper (`PacingStrategy`), `AbandonmentRatePacingConfig$targetRate` (`@range 0..1`) and `$evaluationWindow` (`@pattern ^PT([1-9]|1[0-9]|2[0-4])H$`).

This crate states — in `state.rs` and in five handlers — that exactly those members do NOT reach a handler already checked, which is why it hand-rolls `communication_limits_violation`, `communication_time_violation`, `source_violation`, `integration_config_violation`, `integration_identifier_violation` and `channel_subtype_parameters_violation`. There is no `channel_subtype_config_violation` anywhere in the crate (`grep -n 'outbound_mode|bandwidth' src/` matches only the generated types), so on the crate's own premise a caller can store `{"telephony": {"outboundMode": {"progressive": {"bandwidthAllocation": -0.5}}, ...}}` at HTTP 200, and `DescribeCampaign` then projects a `Campaign$channelSubtypeConfig` the model's own shape forbids — precisely the failure mode `state.rs:419-425` says the sub-walk exists to prevent.

The two sides of the crate cannot both be right, and either way one of them must change: the crate's own suite already asserts those seven values are refused with HTTP 400 `ValidationException` and that the stored config is left alone (tests/all/campaign_channel_subtype_config_update.rs:611-677, asserted at :714-731). So either the handler has to compensate for the union-borne constraints the way its five siblings do, or the premise restated in six places — the premise five existing hand-rolled checks are justified by — is wrong and those checks are dead code. A fixer can tell which by running that test file; the behaviour to establish is the one the test already states.

## Evidence

The crate's premise, services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/state.rs:415-425:

```
// A constrained member reaches a handler already checked — unless a UNION stands
// between it and the root of the request, because the generated member schemas of a
// union carry no constraint traits, so the centralized walk has nothing to enforce
// there and does not descend (the same reason the two checks above exist). The value
// behind the union member is therefore judged HERE by that very walk, started one
// level down ... so no record can come to hold — and no read project — an ARN the
// model's own shape forbids.
```

The handler, which judges nothing — src/ops/update_campaign_channel_subtype_config.rs:175-218 (the whole `handle` body is the store lookup plus):

```
        campaign.channel_subtype_config = Some(input.channel_subtype_config);
```

and its own doc, src/ops/update_campaign_channel_subtype_config.rs:17-20: "the update turns on the campaign existing in this region and on nothing else." The create path stores it unchecked too — src/ops/create_campaign.rs:339 `channel_subtype_config: input.channel_subtype_config,` — although create_campaign.rs:183-250 runs `communication_limits_violation`, `communication_time_violation`, `source_violation` and the tag checks first, each with a comment naming the union as the reason.

The model's constraints behind that union (`smithy_query(query="closure", operation="UpdateCampaignChannelSubtypeConfig")`):

```
"TelephonyOutboundMode": {"type": "union", "members": {"progressive": "ProgressiveConfig", "predictive": "PredictiveConfig", "agentless": "AgentlessConfig", "preview": "PreviewConfig"}}
"ProgressiveConfig": {"bandwidthAllocation": {"target": "BandwidthAllocation", "required": true, "range": {"min": 0, "max": 2}}}
"PredictiveConfig": {"pacingStrategies": {"target": "PacingStrategyList", "length": {"min": 1, "max": 1}}}
"TimeoutConfig": {"durationInSeconds": {"target": "TimeoutDuration", "required": true, "range": {"min": 1, "max": 300}}}
"AbandonmentRatePacingConfig": {"evaluationWindow": {"target": "EvaluationWindow", "required": true, "length": {"max": 5}, "pattern": "^PT([1-9]|1[0-9]|2[0-4])H$"}}
```

The other side of the inconsistency — the crate's own suite demands those refusals. tests/all/campaign_channel_subtype_config_update.rs:611-618:

```
        (
            "bandwidthAllocation one step below the 0 minimum",
            telephony(json!({"progressive": {"bandwidthAllocation": -0.5}})),
        ),
        (
            "bandwidthAllocation one step above the 2 maximum",
            telephony(json!({"predictive": {"bandwidthAllocation": 2.5}})),
        ),
```

plus the rows at :629-677 ("a preview timeout one step below the 1 second minimum", "two pacing strategies where the list holds one", "an agent action the catalog does not hold", "an evaluationWindow the pattern does not admit"), all asserted at :714-731:

```
    for (case, config) in &refused {
        let (status, response) = update_subtype_config(&router, region, &id, config).await;
        assert_eq!(status, 400, "case {case:?}: expected HTTP 400, got {status} {response}");
```

No compensating helper exists: `grep -n "outbound_mode|bandwidth|OutboundMode|channel_subtype_config_violation|pacing" src/` matches only src/ops/types/shared_types.rs (the generated types), never state.rs or a handler.

## Steps to Reproduce

1. Register a Connect instance and run `StartInstanceOnboardingJob`, waiting out `crate::state::ONBOARDING_JOB_DURATION`; `CreateCampaign` to get an id.
2. `POST /v2/campaigns/{id}/channel-subtype-config` with `{"channelSubtypeConfig": {"telephony": {"outboundMode": {"progressive": {"bandwidthAllocation": -0.5}}, "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}}}}` — `bandwidthAllocation` is below `BandwidthAllocation`'s `@range` minimum of 0.
3. Compare the answer with what the crate requires of itself: tests/all/campaign_channel_subtype_config_update.rs:611-618 + :714-731 demand HTTP 400 `ValidationException` with the stored config untouched, while src/ops/update_campaign_channel_subtype_config.rs:175-218 contains no check that could produce it and src/state.rs:415-425 states the centralized walk does not descend past the `outboundMode` union.
4. If the call answers 200, `GET /v2/campaigns/{id}` and observe `channelSubtypeConfig.telephony.outboundMode.progressive.bandwidthAllocation == -0.5` persisted and projected — a `Campaign` outside the model's own shape — and add the missing check beside its five siblings (`crate::state::source_violation`, `channel_subtype_parameters_violation`, …).
5. Repeat for the rows at :629-677 (`timeoutConfig.durationInSeconds: 0`, two `pacingStrategies`, `agentActions: ["KEEP"]`, `evaluationWindow: "PT25H"`) and for the same member on `PUT /v2/campaigns` (src/ops/create_campaign.rs:339), which stores it the same unchecked way.
