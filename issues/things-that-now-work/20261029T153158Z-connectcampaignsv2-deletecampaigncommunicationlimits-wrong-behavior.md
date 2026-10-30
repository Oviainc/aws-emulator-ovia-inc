Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: bdfeee3e6512ca319f6cb10a0fb27e2039f01e12

# DeleteCampaignCommunicationLimits discards the whole communicationLimitsOverride on a delete that removed nothing, against its own "left as it was" comment and the sibling clear path's guard

_Filed: 2026-10-29T15:31:58Z_
_Operation: DeleteCampaignCommunicationLimits | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `bdfeee3e6512ca319f6cb10a0fb27e2039f01e12`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`clear_communication_limits` collapses the campaign's whole `communicationLimitsOverride` to `None` whenever both of its members end up absent — including when the named block (`allChannelSubtypes`) was ALREADY absent, i.e. when the delete removed nothing at all.

The model allows a caller to store an empty override: `CommunicationLimitsConfig` has two OPTIONAL members (`allChannelSubtypes`, `instanceLimitsHandling`), and `UpdateCampaignCommunicationLimits` writes the request's value verbatim (`campaign.communication_limits_override = Some(input.communication_limits_override);`, update_campaign_communication_limits.rs:253). So after `POST /v2/campaigns/{id}/communication-limits` with `{"communicationLimitsOverride": {}}`, `DescribeCampaign` projects `"communicationLimitsOverride": {}`. A subsequent `DELETE /v2/campaigns/{id}/communication-limits?config=ALL_CHANNEL_SUBTYPES` removes nothing (there is no `allChannelSubtypes` block) yet WRITES to the record: the member disappears from every later `DescribeCampaign` response.

Two places in this crate say that must not happen:

1. The handler's own comment directly above the call states the rule it breaks: "A campaign that held none is left as it was, which is what makes the operation idempotent, as the model declares." A campaign whose override holds no `allChannelSubtypes` block *held none*, and it is not left as it was.
2. The sibling clear path for the same kind of deletion, `clear_channel_subtype_config`, guards exactly this case with a `removed` flag and spells out why: "Only a removal collapses the config: a call that removed nothing writes nothing at all, so a campaign carrying a config with no channel in it keeps that config rather than losing it to a deletion that found nothing." `clear_communication_limits` skips that guard.

The collapse itself is correct when the delete DID remove a block; the defect is that the collapse is not conditioned on something having been removed, so the deletion mutates a record it documents itself as leaving untouched.

## Evidence

The sibling path guards the collapse — services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/state.rs:1373-1382 (`StoredCampaign::clear_channel_subtype_config`):

```
        // Only a removal collapses the config: a call that removed nothing writes
        // nothing at all, so a campaign carrying a config with no channel in it keeps
        // that config rather than losing it to a deletion that found nothing.
        if removed
            && config.email.is_none()
            && config.sms.is_none()
            && config.telephony.is_none()
            && config.whats_app.is_none()
        {
            self.channel_subtype_config = None;
        }
```

The path under review does not — src/state.rs:1430-1443 (`StoredCampaign::clear_communication_limits`), with no `removed` flag anywhere:

```
    pub fn clear_communication_limits(&mut self, config_type: &str) {
        let Some(config) = self.communication_limits_override.as_mut() else {
            return;
        };
        match config_type {
            "ALL_CHANNEL_SUBTYPES" => config.all_channel_subtypes = None,
            // A catalog value with no block on `CommunicationLimitsConfig` is a config
            // this emulator cannot be configured for, so it holds nothing to remove.
            _ => return,
        }
        if config.all_channel_subtypes.is_none() && config.instance_limits_handling.is_none() {
            self.communication_limits_override = None;
        }
    }
```

The handler comment that the code breaks — src/ops/delete_campaign_communication_limits.rs:263-267:

```
        // The whole of the operation's effect: the campaign no longer holds a
        // communication-limits block for the config the request named. A campaign that
        // held none is left as it was, which is what makes the operation idempotent, as
        // the model declares.
        campaign.clear_communication_limits(&input.config);
```

The empty override is model-allowed input: `smithy_query(query="shape", shape="CommunicationLimitsConfig")` returns `{"type": "structure", "members": {"allChannelSubtypes": {"target": "CommunicationLimits"}, "instanceLimitsHandling": {"target": "InstanceLimitsHandling"}}}` — neither member is `@required` — and the update stores it verbatim at src/ops/update_campaign_communication_limits.rs:253: `campaign.communication_limits_override = Some(input.communication_limits_override);`

Test coverage: no test exercises an empty override. tests/all/campaign_communication_limits_deletion.rs and tests/all/campaign_configuration_is_one_rule.rs:92-103 only ever send a populated `{"allChannelSubtypes": {"communicationLimitsList": [...]}}`, so nothing pins the behaviour either way; tests/all/campaign_channel_subtype_config_deletion.rs is the suite that pins the sibling's `removed` guard.

## Steps to Reproduce

1. Register a Connect instance, run `StartInstanceOnboardingJob` and wait out `crate::state::ONBOARDING_JOB_DURATION`, then `PUT /v2/campaigns` to mint a campaign id.
2. `POST /v2/campaigns/{id}/communication-limits` with body `{"communicationLimitsOverride": {}}` — both members of `CommunicationLimitsConfig` are optional, so this is accepted (HTTP 200).
3. `GET /v2/campaigns/{id}` and observe `campaign.communicationLimitsOverride == {}` — the campaign now holds an override with no `allChannelSubtypes` block.
4. `DELETE /v2/campaigns/{id}/communication-limits?config=ALL_CHANNEL_SUBTYPES` — HTTP 200. There was no `allChannelSubtypes` block to remove, so per src/ops/delete_campaign_communication_limits.rs:263-266 ("A campaign that held none is left as it was") the record must be untouched.
5. `GET /v2/campaigns/{id}` again and observe `campaign.communicationLimitsOverride` is now ABSENT: the delete that removed nothing wrote to the record, which is exactly what the sibling `clear_channel_subtype_config` guards against at src/state.rs:1373-1376.
6. Contrast with the guarded sibling: create a campaign with `"channelSubtypeConfig": {}`-shaped state reachable via `POST /v2/campaigns/{id}/channel-subtype-config`, then `DELETE /v2/campaigns/{id}/channel-subtype-config?channelSubtype=TELEPHONY` — that path leaves the config in place because nothing was removed.
